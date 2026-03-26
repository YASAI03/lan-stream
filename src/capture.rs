use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::watch;
use windows::{
    core::*,
    Graphics::Capture::*,
    Graphics::DirectX::DirectXPixelFormat,
    Graphics::SizeInt32,
    Win32::Graphics::Direct3D::*,
    Win32::Graphics::Direct3D11::*,
    Win32::Graphics::Dxgi::Common::*,
    Win32::Graphics::Dxgi::*,
    Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
    Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop,
    Win32::UI::WindowsAndMessaging::*,
    Win32::Foundation::*,
};
use std::sync::mpsc;

#[derive(Debug, Clone, serde::Serialize)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub title: String,
}

/// Enumerate visible windows with non-empty titles
pub fn enum_windows() -> Vec<WindowInfo> {
    let mut windows = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(enum_windows_proc),
            LPARAM(&raw mut windows as isize),
        );
    }
    windows
}

unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        let windows = &mut *(lparam.0 as *mut Vec<WindowInfo>);

        if !IsWindowVisible(hwnd).as_bool() {
            return TRUE;
        }

        let mut title = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut title);
        if len == 0 {
            return TRUE;
        }

        let title = String::from_utf16_lossy(&title[..len as usize]);
        if title.is_empty() {
            return TRUE;
        }

        windows.push(WindowInfo {
            hwnd: hwnd.0 as isize,
            title,
        });
        TRUE
    }
}

/// Find a window by partial title match
pub fn find_window_by_title(title: &str) -> Option<HWND> {
    if title.is_empty() {
        return None;
    }
    let title_lower = title.to_lowercase();
    enum_windows()
        .into_iter()
        .find(|w| w.title.to_lowercase().contains(&title_lower))
        .map(|w| HWND(w.hwnd as *mut _))
}

/// Create a D3D11 device
fn create_d3d11_device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    Ok((device.unwrap(), context.unwrap()))
}

/// Convert ID3D11Device to WinRT IDirect3DDevice
fn create_direct3d_device(
    d3d_device: &ID3D11Device,
) -> Result<windows::Graphics::DirectX::Direct3D11::IDirect3DDevice> {
    unsafe {
        let dxgi_device: IDXGIDevice = d3d_device.cast()?;
        let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)?;
        inspectable.cast()
    }
}

/// Create a staging texture for CPU readback
fn create_staging_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    unsafe {
        let mut texture = None;
        device.CreateTexture2D(&desc, None, Some(&mut texture))?;
        Ok(texture.unwrap())
    }
}

/// Undo premultiplied alpha and convert BGRA → RGBA in-place.
fn unpremultiply_bgra_to_rgba(data: &mut [u8]) {
    static RECIP: std::sync::LazyLock<[u16; 256]> = std::sync::LazyLock::new(|| {
        let mut table = [0u16; 256];
        for a in 1..256 {
            table[a] = ((255 * 256) / a) as u16;
        }
        table
    });
    let recip = &*RECIP;

    for pixel in data.chunks_exact_mut(4) {
        let a = pixel[3] as usize;
        if a == 255 {
            pixel.swap(0, 2);
        } else if a == 0 {
            pixel[0] = 0;
            pixel[1] = 0;
            pixel[2] = 0;
        } else {
            let r = recip[a];
            let b = ((pixel[0] as u16 * r) >> 8).min(255) as u8;
            let g = ((pixel[1] as u16 * r) >> 8).min(255) as u8;
            let rv = ((pixel[2] as u16 * r) >> 8).min(255) as u8;
            pixel[0] = rv;
            pixel[1] = g;
            pixel[2] = b;
        }
    }
}

/// Encode BGRA pixel data to QOI with alpha.
fn encode_qoi(bgra_data: &mut [u8], width: u32, height: u32) -> Vec<u8> {
    unpremultiply_bgra_to_rgba(bgra_data);
    qoi::encode_to_vec(bgra_data, width, height).unwrap_or_default()
}

/// Start the capture loop in a dedicated thread.
/// Sends QOI frames via the watch channel.
pub fn start_capture_thread(
    frame_tx: watch::Sender<Arc<Vec<u8>>>,
    config: crate::config::SharedConfig,
    debug: crate::debug::DebugStore,
    stop_signal: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        if let Err(e) = run_capture_loop(frame_tx, config, &debug, &stop_signal) {
            debug.push_log(format!("Capture error: {e}"));
            eprintln!("Capture error: {e}");
        }
    })
}

fn run_capture_loop(
    frame_tx: watch::Sender<Arc<Vec<u8>>>,
    config: crate::config::SharedConfig,
    debug: &crate::debug::DebugStore,
    stop_signal: &AtomicBool,
) -> Result<()> {
    loop {
        if stop_signal.load(Ordering::SeqCst) {
            return Ok(());
        }
        // Read current config
        let (window_title, fps, capture_cursor) = {
            let cfg = config.blocking_read();
            (cfg.capture.window_title.clone(), cfg.capture.target_fps, cfg.capture.capture_cursor)
        };

        if window_title.is_empty() {
            std::thread::sleep(std::time::Duration::from_secs(1));
            continue;
        }

        let hwnd = match find_window_by_title(&window_title) {
            Some(h) => h,
            None => {
                let msg = format!("Window not found: \"{window_title}\", retrying...");
                debug.push_log(msg.clone());
                eprintln!("{msg}");
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };

        {
            let msg = format!("Capturing window: \"{window_title}\"");
            debug.push_log(msg.clone());
            eprintln!("{msg}");
        }

        match run_capture_session(hwnd, fps, capture_cursor, &frame_tx, &config, debug, stop_signal) {
            Ok(()) => {} // session ended cleanly (config changed)
            Err(e) => {
                let msg = format!("Capture session error: {e}, restarting...");
                debug.push_log(msg.clone());
                eprintln!("{msg}");
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    }
}

/// Frame data sent from capture thread to encode thread
struct RawFrame {
    bgra_data: Vec<u8>,
    width: u32,
    height: u32,
    t0: std::time::Instant,
    t_copy: std::time::Duration,
    t_map: std::time::Duration,
    t_readback: std::time::Duration,
}

fn run_capture_session(
    hwnd: HWND,
    fps: u32,
    capture_cursor: bool,
    frame_tx: &watch::Sender<Arc<Vec<u8>>>,
    config: &crate::config::SharedConfig,
    debug: &crate::debug::DebugStore,
    stop_signal: &AtomicBool,
) -> Result<()> {
    let (d3d_device, d3d_context) = create_d3d11_device()?;
    let direct3d_device = create_direct3d_device(&d3d_device)?;

    // Create capture item from window handle
    let interop: IGraphicsCaptureItemInterop =
        windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd)? };

    let size = item.Size()?;
    let width = size.Width as u32;
    let height = size.Height as u32;

    if width == 0 || height == 0 {
        return Err(Error::new(E_FAIL, "Window has zero size"));
    }

    // Create frame pool with 2 buffers
    let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &direct3d_device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2,
        SizeInt32 {
            Width: size.Width,
            Height: size.Height,
        },
    )?;

    let session = frame_pool.CreateCaptureSession(&item)?;
    let _ = session.SetIsBorderRequired(false);
    let _ = session.SetIsCursorCaptureEnabled(capture_cursor);

    // FrameArrived callback
    let frame_event = Arc::new(std::sync::Condvar::new());
    let frame_mutex = Arc::new(std::sync::Mutex::new(false));
    {
        let event = frame_event.clone();
        let mutex = frame_mutex.clone();
        frame_pool.FrameArrived(&windows::Foundation::TypedEventHandler::new(
            move |_pool, _args| {
                let mut arrived = mutex.lock().unwrap();
                *arrived = true;
                event.notify_one();
                Ok(())
            },
        ))?;
    }

    session.StartCapture()?;

    let mut frame_interval = std::time::Duration::from_millis((1000 / fps.max(1)) as u64);
    let mut current_fps = fps;
    let mut current_title = config.blocking_read().capture.window_title.clone();

    // Double-buffered staging textures
    let mut staging_textures: [Option<ID3D11Texture2D>; 2] = [None, None];
    let mut staging_idx: usize = 0;

    // Pre-allocate two pixel buffers for double-buffering
    let buf_capacity = (width * height * 4) as usize;
    let mut bgra_buffers: [Vec<u8>; 2] = [
        Vec::with_capacity(buf_capacity),
        Vec::with_capacity(buf_capacity),
    ];

    // Channel for capture→encode pipeline (bounded=1 to limit latency)
    let (raw_tx, raw_rx) = mpsc::sync_channel::<RawFrame>(1);

    // Spawn encode thread
    let encode_frame_tx = frame_tx.clone();
    let encode_debug = debug.clone();
    let encode_handle = std::thread::spawn(move || {
        let mut frame_count: u64 = 0;
        let mut log_timer = std::time::Instant::now();

        while let Ok(mut raw) = raw_rx.recv() {
            let encoded = encode_qoi(&mut raw.bgra_data, raw.width, raw.height);
            let t_encode = raw.t0.elapsed();

            if !encoded.is_empty() {
                let _ = encode_frame_tx.send(Arc::new(encoded));
            }

            frame_count += 1;
            if log_timer.elapsed() >= std::time::Duration::from_secs(3) {
                let fps_actual = frame_count as f64 / log_timer.elapsed().as_secs_f64();
                let gpu_copy_ms = raw.t_copy.as_secs_f64() * 1000.0;
                let map_ms = (raw.t_map - raw.t_copy).as_secs_f64() * 1000.0;
                let readback_ms = (raw.t_readback - raw.t_map).as_secs_f64() * 1000.0;
                let encode_ms = (t_encode - raw.t_readback).as_secs_f64() * 1000.0;
                let total_ms = raw.t0.elapsed().as_secs_f64() * 1000.0;
                eprintln!(
                    "[perf] {}x{} fps={fps_actual:.1} | gpu_copy={gpu_copy_ms:.1}ms map={map_ms:.1}ms readback={readback_ms:.1}ms encode={encode_ms:.1}ms total={total_ms:.1}ms",
                    raw.width, raw.height,
                );
                encode_debug.push_metrics(raw.width, raw.height, fps_actual, gpu_copy_ms, map_ms, readback_ms, encode_ms, total_ms);
                frame_count = 0;
                log_timer = std::time::Instant::now();
            }
        }
    });

    let mut config_check_timer = std::time::Instant::now();

    let result = (|| -> Result<()> {
        loop {
            let loop_start = std::time::Instant::now();

            // Check config every 1 second
            if config_check_timer.elapsed() >= std::time::Duration::from_secs(1) {
                if stop_signal.load(Ordering::SeqCst) {
                    session.Close()?;
                    frame_pool.Close()?;
                    return Ok(());
                }
                let cfg = config.blocking_read();
                if cfg.capture.window_title != current_title {
                    session.Close()?;
                    frame_pool.Close()?;
                    return Ok(());
                }
                if cfg.capture.target_fps != current_fps {
                    current_fps = cfg.capture.target_fps;
                    frame_interval = std::time::Duration::from_millis(
                        (1000 / current_fps.max(1)) as u64,
                    );
                    let msg = format!("FPS changed to {current_fps}");
                    debug.push_log(msg.clone());
                    eprintln!("{msg}");
                }
                current_title = cfg.capture.window_title.clone();
                config_check_timer = std::time::Instant::now();
            }

            // Wait for frame arrival
            {
                let mut arrived = frame_mutex.lock().unwrap();
                if !*arrived {
                    let result = frame_event.wait_timeout(arrived, frame_interval).unwrap();
                    arrived = result.0;
                }
                *arrived = false;
            }

            // Try to get a frame
            if let Ok(frame) = frame_pool.TryGetNextFrame() {
                let t0 = std::time::Instant::now();

                let surface = frame.Surface()?;
                let frame_size = frame.ContentSize()?;
                let fw = frame_size.Width as u32;
                let fh = frame_size.Height as u32;

                let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
                let source_texture: ID3D11Texture2D = unsafe { access.GetInterface()? };

                // Get or create staging texture (double-buffered)
                let idx = staging_idx;
                staging_idx = 1 - staging_idx;

                let staging = match &staging_textures[idx] {
                    Some(t) => {
                        let mut desc = D3D11_TEXTURE2D_DESC::default();
                        unsafe { t.GetDesc(&mut desc) };
                        if desc.Width != fw || desc.Height != fh {
                            let new_staging = create_staging_texture(&d3d_device, fw, fh)?;
                            staging_textures[idx] = Some(new_staging);
                            staging_textures[idx].as_ref().unwrap()
                        } else {
                            t
                        }
                    }
                    None => {
                        staging_textures[idx] = Some(create_staging_texture(&d3d_device, fw, fh)?);
                        staging_textures[idx].as_ref().unwrap()
                    }
                };

                // GPU copy
                unsafe {
                    d3d_context.CopyResource(staging, &source_texture);
                }
                let t_copy = t0.elapsed();

                // Map
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                unsafe {
                    d3d_context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
                }
                let t_map = t0.elapsed();

                // Readback into reusable buffer
                let row_bytes = (fw * 4) as usize;
                let total_bytes = row_bytes * fh as usize;
                let buf = &mut bgra_buffers[idx];
                buf.clear();
                if buf.capacity() < total_bytes {
                    buf.reserve(total_bytes - buf.capacity());
                }

                if mapped.RowPitch as usize == row_bytes {
                    let src = unsafe {
                        std::slice::from_raw_parts(mapped.pData as *const u8, total_bytes)
                    };
                    buf.extend_from_slice(src);
                } else {
                    for row in 0..fh as usize {
                        let src = unsafe {
                            std::slice::from_raw_parts(
                                (mapped.pData as *const u8).add(row * mapped.RowPitch as usize),
                                row_bytes,
                            )
                        };
                        buf.extend_from_slice(src);
                    }
                }

                unsafe {
                    d3d_context.Unmap(staging, 0);
                }
                let t_readback = t0.elapsed();

                // Send to encode thread (non-blocking: swap buffer ownership)
                let send_buf = std::mem::replace(buf, Vec::with_capacity(total_bytes));
                let _ = raw_tx.try_send(RawFrame {
                    bgra_data: send_buf,
                    width: fw,
                    height: fh,
                    t0,
                    t_copy,
                    t_map,
                    t_readback,
                });
            }

            // Frame rate limiting
            let elapsed = loop_start.elapsed();
            if elapsed < frame_interval {
                std::thread::sleep(frame_interval - elapsed);
            }
        }
    })();

    // Drop sender to signal encode thread to exit
    drop(raw_tx);
    let _ = encode_handle.join();

    result
}
