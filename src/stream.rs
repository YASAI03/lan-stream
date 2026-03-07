use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use axum::body::Body;
use axum::response::{IntoResponse, Response};
use axum::http::StatusCode;
use tokio::sync::watch;

static STREAM_ACTIVE: AtomicBool = AtomicBool::new(false);

const BOUNDARY: &str = "frame";

/// MJPEG stream handler. Only one client at a time.
pub async fn mjpeg_stream(
    frame_rx: watch::Receiver<Arc<Vec<u8>>>,
    fps: u32,
) -> Response {
    // Check single-client limit
    if STREAM_ACTIVE.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Another client is already connected",
        ).into_response();
    }

    let frame_interval = std::time::Duration::from_millis((1000 / fps.max(1)) as u64);

    let stream = async_stream::stream! {
        let mut rx = rx_clone(frame_rx);
        let _guard = StreamGuard;

        loop {
            // Wait for a new frame
            if rx.changed().await.is_err() {
                break;
            }

            let frame = rx.borrow_and_update().clone();
            if frame.is_empty() {
                continue;
            }

            // Build MJPEG multipart chunk
            let header = format!(
                "--{BOUNDARY}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                frame.len()
            );

            yield Ok::<_, std::io::Error>(axum::body::Bytes::from(header));
            yield Ok::<_, std::io::Error>(axum::body::Bytes::from(frame.as_ref().clone()));
            yield Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"\r\n"));

            tokio::time::sleep(frame_interval).await;
        }
    };

    let body = Body::from_stream(stream);

    Response::builder()
        .header("Content-Type", format!("multipart/x-mixed-replace; boundary={BOUNDARY}"))
        .header("Cache-Control", "no-cache, no-store, must-revalidate")
        .header("Pragma", "no-cache")
        .header("Expires", "0")
        .body(body)
        .unwrap()
        .into_response()
}

fn rx_clone(rx: watch::Receiver<Arc<Vec<u8>>>) -> watch::Receiver<Arc<Vec<u8>>> {
    rx
}

/// Guard to reset STREAM_ACTIVE when the stream drops
struct StreamGuard;

impl Drop for StreamGuard {
    fn drop(&mut self) {
        STREAM_ACTIVE.store(false, Ordering::SeqCst);
    }
}
