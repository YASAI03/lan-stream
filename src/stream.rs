use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use axum::extract::ws::{Message, WebSocket};
use tokio::sync::broadcast;

use crate::debug::DebugStore;

static CLIENT_COUNT: AtomicUsize = AtomicUsize::new(0);
static TOTAL_BITRATE_BPS: AtomicU64 = AtomicU64::new(0);

pub fn client_count() -> usize {
    CLIENT_COUNT.load(Ordering::Relaxed)
}

pub fn total_bitrate_bps() -> u64 {
    TOTAL_BITRATE_BPS.load(Ordering::Relaxed)
}

/// WebSocket stream handler. Supports multiple clients.
pub async fn ws_stream(
    mut socket: WebSocket,
    frame_tx: broadcast::Sender<Arc<Vec<u8>>>,
    _fps: u32,
    debug: DebugStore,
) {
    let mut guard = ClientGuard::new();
    debug.push_log(format!("Client connected (total: {})", client_count()));

    let mut rx = frame_tx.subscribe();
    // New clients must wait for a keyframe before delta frames make sense
    let mut need_keyframe = true;
    let mut bytes_since_report: u64 = 0;
    let mut last_report = tokio::time::Instant::now();

    loop {
        let frame = match rx.recv().await {
            Ok(frame) => frame,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                // Missed frames — delta chain is broken, skip until next keyframe
                debug.push_log(format!("Client lagged, missed {n} frames, waiting for keyframe"));
                need_keyframe = true;
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => break,
        };

        if frame.is_empty() {
            continue;
        }

        // Skip delta frames until we receive a keyframe to resync
        if need_keyframe {
            if frame.first() == Some(&0x00) {
                need_keyframe = false;
            } else {
                continue;
            }
        }

        let len = frame.len() as u64;
        if socket.send(Message::Binary(frame.as_ref().clone().into())).await.is_err() {
            break;
        }

        bytes_since_report += len;
        let now = tokio::time::Instant::now();
        let elapsed = now.duration_since(last_report);
        if elapsed >= std::time::Duration::from_secs(1) {
            let ms = elapsed.as_millis().max(1) as u64;
            let new_bitrate = bytes_since_report * 8 * 1000 / ms;
            guard.update_bitrate(new_bitrate);
            bytes_since_report = 0;
            last_report = now;

            let msg = format!(r#"{{"bitrate_bps":{}}}"#, new_bitrate);
            if socket.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    }

    let bps = guard.bitrate;
    drop(guard);
    debug.push_log(format!("Client disconnected (was {} bps, remaining: {})", bps, client_count()));
}

/// Guard to track client count and per-client bitrate contribution
struct ClientGuard {
    bitrate: u64,
}

impl ClientGuard {
    fn new() -> Self {
        CLIENT_COUNT.fetch_add(1, Ordering::Relaxed);
        Self { bitrate: 0 }
    }

    fn update_bitrate(&mut self, new_bitrate: u64) {
        TOTAL_BITRATE_BPS.fetch_sub(self.bitrate, Ordering::Relaxed);
        self.bitrate = new_bitrate;
        TOTAL_BITRATE_BPS.fetch_add(self.bitrate, Ordering::Relaxed);
    }
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        TOTAL_BITRATE_BPS.fetch_sub(self.bitrate, Ordering::Relaxed);
        CLIENT_COUNT.fetch_sub(1, Ordering::Relaxed);
    }
}
