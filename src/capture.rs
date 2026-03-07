use std::sync::Arc;
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

/// Encode BGRA pixel data to JPEG using libjpeg-turbo (SIMD-accelerated)
fn encode_jpeg(bgra_data: &[u8], width: u32, height: u32, quality: u8) -> Vec<u8> {
    let image = turbojpeg::Image {
        pixels: bgra_data,
        width: width as usize,
        pitch: width as usize * 4,
        height: height as usize,
        format: turbojpeg::PixelFormat::BGRA,
    };
    let mut compressor = turbojpeg::Compressor::new().expect("failed to create JPEG compressor");
    let _ = compressor.set_quality(quality as i32);
    let _ = compressor.set_subsamp(turbojpeg::Subsamp::Sub2x2);
    compressor.compress_to_vec(image).unwrap_or_default()
}

/// Start the capture loop in a dedicated thread.
/// Sends JPEG frames via the watch channel.
pub fn start_capture_thread(
    frame_tx: watch::Sender<Arc<Vec<u8>>>,
    config: crate::config::SharedConfig,
    debug: crate::debug::DebugStore,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        // Graphics Capture API requires a dispatcher queue on this thread
        if let Err(e) = run_capture_loop(frame_tx, config, &debug) {
            debug.push_log(format!("Capture error: {e}"));
            eprintln!("Capture error: {e}");
        }
    })
}

fn run_capture_loop(
    frame_tx: watch::Sender<Arc<Vec<u8>>>,
    config: crate::config::SharedConfig,
    debug: &crate::debug::DebugStore,
) -> Result<()> {
    loop {
        // Read current config
        let (window_title, fps, quality, capture_cursor) = {
            let cfg = config.blocking_read();
            (cfg.capture.window_title.clone(), cfg.capture.target_fps, cfg.capture.quality, cfg.capture.capture_cursor)
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

        match run_capture_session(hwnd, fps, quality, capture_cursor, &frame_tx, &config, debug) {
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

fn run_capture_session(
    hwnd: HWND,
    fps: u32,
    quality: u8,
    capture_cursor: bool,
    frame_tx: &watch::Sender<Arc<Vec<u8>>>,
    config: &crate::config::SharedConfig,
    debug: &crate::debug::DebugStore,
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

    // Create frame pool
    let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &direct3d_device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        1,
        SizeInt32 {
            Width: size.Width,
            Height: size.Height,
        },
    )?;

    let session = frame_pool.CreateCaptureSession(&item)?;

    // Disable yellow capture border (Windows 11+, ignore error on older)
    let _ = session.SetIsBorderRequired(false);

    // Set cursor capture based on config
    let _ = session.SetIsCursorCaptureEnabled(capture_cursor);

    session.StartCapture()?;

    let frame_interval = std::time::Duration::from_millis((1000 / fps.max(1)) as u64);
    let mut staging_texture: Option<ID3D11Texture2D> = None;
    let mut current_title = {
        config.blocking_read().capture.window_title.clone()
    };

    // Performance logging
    let mut frame_count: u64 = 0;
    let mut log_timer = std::time::Instant::now();

    loop {
        let loop_start = std::time::Instant::now();

        // Check if config changed (window title)
        {
            let cfg = config.blocking_read();
            if cfg.capture.window_title != current_title {
                // Config changed, restart capture session
                session.Close()?;
                frame_pool.Close()?;
                return Ok(());
            }
            current_title = cfg.capture.window_title.clone();
        }

        // Try to get a frame
        if let Ok(frame) = frame_pool.TryGetNextFrame() {
            let t0 = std::time::Instant::now();

            let surface = frame.Surface()?;
            let frame_size = frame.ContentSize()?;
            let fw = frame_size.Width as u32;
            let fh = frame_size.Height as u32;

            // Get the D3D11 texture from the surface
            let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
            let source_texture: ID3D11Texture2D = unsafe { access.GetInterface()? };

            // Create/recreate staging texture if needed
            let staging = match &staging_texture {
                Some(t) => {
                    let mut desc = D3D11_TEXTURE2D_DESC::default();
                    unsafe { t.GetDesc(&mut desc) };
                    if desc.Width != fw || desc.Height != fh {
                        let new_staging = create_staging_texture(&d3d_device, fw, fh)?;
                        staging_texture = Some(new_staging);
                        staging_texture.as_ref().unwrap()
                    } else {
                        t
                    }
                }
                None => {
                    staging_texture = Some(create_staging_texture(&d3d_device, fw, fh)?);
                    staging_texture.as_ref().unwrap()
                }
            };

            // Copy from GPU texture to staging texture
            unsafe {
                d3d_context.CopyResource(staging, &source_texture);
            }
            let t_copy = t0.elapsed();

            // Map staging texture for CPU read
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            unsafe {
                d3d_context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            }
            let t_map = t0.elapsed();

            // Copy pixel data (respecting row pitch)
            let row_bytes = (fw * 4) as usize;
            let mut bgra_data = Vec::with_capacity((fw * fh * 4) as usize);
            for row in 0..fh as usize {
                let src =
                    unsafe { std::slice::from_raw_parts(
                        (mapped.pData as *const u8).add(row * mapped.RowPitch as usize),
                        row_bytes,
                    ) };
                bgra_data.extend_from_slice(src);
            }

            unsafe {
                d3d_context.Unmap(staging, 0);
            }
            let t_readback = t0.elapsed();

            // Encode to JPEG
            let jpeg = encode_jpeg(&bgra_data, fw, fh, quality);
            let t_encode = t0.elapsed();

            if !jpeg.is_empty() {
                let _ = frame_tx.send(Arc::new(jpeg));
            }

            frame_count += 1;
            if log_timer.elapsed() >= std::time::Duration::from_secs(3) {
                let fps_actual = frame_count as f64 / log_timer.elapsed().as_secs_f64();
                let gpu_copy_ms = t_copy.as_secs_f64() * 1000.0;
                let map_ms = (t_map - t_copy).as_secs_f64() * 1000.0;
                let readback_ms = (t_readback - t_map).as_secs_f64() * 1000.0;
                let encode_ms = (t_encode - t_readback).as_secs_f64() * 1000.0;
                let total_ms = t0.elapsed().as_secs_f64() * 1000.0;
                eprintln!(
                    "[perf] {fw}x{fh} fps={fps_actual:.1} | gpu_copy={gpu_copy_ms:.1}ms map={map_ms:.1}ms readback={readback_ms:.1}ms encode={encode_ms:.1}ms total={total_ms:.1}ms",
                );
                debug.push_metrics(fw, fh, fps_actual, gpu_copy_ms, map_ms, readback_ms, encode_ms, total_ms);
                frame_count = 0;
                log_timer = std::time::Instant::now();
            }
        }

        // Maintain frame rate
        let elapsed = loop_start.elapsed();
        if elapsed < frame_interval {
            std::thread::sleep(frame_interval - elapsed);
        }
    }
}
