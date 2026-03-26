use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use axum::extract::ws::{Message, WebSocket};
use tokio::sync::watch;

static STREAM_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn is_stream_active() -> bool {
    STREAM_ACTIVE.load(Ordering::SeqCst)
}

/// WebSocket stream handler. Only one client at a time.
pub async fn ws_stream(
    mut socket: WebSocket,
    frame_rx: watch::Receiver<Arc<Vec<u8>>>,
    fps: u32,
) {
    if STREAM_ACTIVE.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return;
    }

    let _guard = StreamGuard;
    let mut rx = frame_rx;
    let frame_interval = std::time::Duration::from_millis((1000 / fps.max(1)) as u64);

    loop {
        if rx.changed().await.is_err() {
            break;
        }

        let frame = rx.borrow_and_update().clone();
        if frame.is_empty() {
            continue;
        }

        if socket.send(Message::Binary(frame.as_ref().clone().into())).await.is_err() {
            break;
        }

        tokio::time::sleep(frame_interval).await;
    }
}

/// Guard to reset STREAM_ACTIVE when the stream drops
struct StreamGuard;

impl Drop for StreamGuard {
    fn drop(&mut self) {
        STREAM_ACTIVE.store(false, Ordering::SeqCst);
    }
}
