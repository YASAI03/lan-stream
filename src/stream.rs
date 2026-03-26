use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use axum::extract::ws::{Message, WebSocket};
use tokio::sync::watch;

static CLIENT_COUNT: AtomicUsize = AtomicUsize::new(0);

pub fn client_count() -> usize {
    CLIENT_COUNT.load(Ordering::Relaxed)
}

/// WebSocket stream handler. Supports multiple clients.
pub async fn ws_stream(
    mut socket: WebSocket,
    frame_rx: watch::Receiver<Arc<Vec<u8>>>,
    fps: u32,
) {
    CLIENT_COUNT.fetch_add(1, Ordering::Relaxed);
    let _guard = ClientGuard;

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

/// Guard to decrement CLIENT_COUNT when the stream drops
struct ClientGuard;

impl Drop for ClientGuard {
    fn drop(&mut self) {
        CLIENT_COUNT.fetch_sub(1, Ordering::Relaxed);
    }
}
