// QOI decoder with delta frame support for WebAssembly
// Raw FFI exports — no wasm-bindgen needed.

use std::cell::UnsafeCell;

struct State {
    input: Vec<u8>,
    output: Vec<u8>, // current decoded frame — also serves as prev_frame for delta
    width: u32,
    height: u32,
}

struct SyncState(UnsafeCell<State>);

// SAFETY: WASM is single-threaded, no concurrent access possible
unsafe impl Sync for SyncState {}

static STATE: SyncState = SyncState(UnsafeCell::new(State {
    input: Vec::new(),
    output: Vec::new(),
    width: 0,
    height: 0,
}));

#[inline(always)]
fn state() -> &'static mut State {
    unsafe { &mut *STATE.0.get() }
}

/// Allocate/resize the input buffer and return a pointer to it.
/// JS should write QOI data into this buffer before calling `decode`.
#[no_mangle]
pub extern "C" fn alloc_input(size: usize) -> *mut u8 {
    let s = state();
    s.input.resize(size, 0);
    s.input.as_mut_ptr()
}

/// Decode a keyframe: full QOI image replaces the current frame buffer.
/// Returns 1 on success, 0 on failure.
#[no_mangle]
pub extern "C" fn decode(input_len: usize) -> u32 {
    let s = state();
    match qoi::decode_to_vec(&s.input[..input_len]) {
        Ok((header, pixels)) => {
            s.width = header.width;
            s.height = header.height;
            s.output = pixels;
            1
        }
        Err(_) => 0,
    }
}

/// Decode a delta frame: QOI-encoded XOR data applied to the previous frame.
/// The output buffer is XOR'd in-place, reconstructing the current frame.
/// Returns 1 on success, 0 on failure (e.g. no previous frame or size mismatch).
#[no_mangle]
pub extern "C" fn decode_delta(input_len: usize) -> u32 {
    let s = state();
    match qoi::decode_to_vec(&s.input[..input_len]) {
        Ok((header, xor_pixels)) => {
            s.width = header.width;
            s.height = header.height;
            if s.output.len() != xor_pixels.len() {
                return 0; // size mismatch — need a keyframe first
            }
            // XOR in-place: output becomes current frame (and prev for next delta)
            for i in 0..xor_pixels.len() {
                s.output[i] ^= xor_pixels[i];
            }
            1
        }
        Err(_) => 0,
    }
}

#[no_mangle]
pub extern "C" fn get_width() -> u32 {
    state().width
}

#[no_mangle]
pub extern "C" fn get_height() -> u32 {
    state().height
}

#[no_mangle]
pub extern "C" fn get_output_ptr() -> *const u8 {
    state().output.as_ptr()
}
