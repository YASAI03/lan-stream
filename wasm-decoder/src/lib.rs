// QOI decoder for WebAssembly
// Raw FFI exports — no wasm-bindgen needed.

use std::cell::UnsafeCell;

struct State {
    input: Vec<u8>,
    output: Vec<u8>,
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

/// Decode the QOI data previously written into the input buffer.
/// Returns 1 on success, 0 on failure.
/// After success, call get_width/get_height/get_output_ptr to read results.
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
