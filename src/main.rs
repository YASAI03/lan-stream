mod capture;
mod config;
mod server;
mod stream;

use std::sync::Arc;
use tokio::sync::{watch, RwLock};

#[tokio::main]
async fn main() {
    let cfg = config::load_config();
    let host = cfg.server.host.clone();
    let port = cfg.server.port;

    let shared_config: config::SharedConfig = Arc::new(RwLock::new(cfg));

    // watch channel: initial empty frame
    let (frame_tx, frame_rx) = watch::channel(Arc::new(Vec::<u8>::new()));

    // Start capture thread
    let capture_config = shared_config.clone();
    let _capture_handle = capture::start_capture_thread(frame_tx, capture_config);

    // Build HTTP server
    let state = server::AppState {
        config: shared_config,
        frame_rx,
    };
    let app = server::create_router(state);

    let addr = format!("{host}:{port}");
    eprintln!("Listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Failed to bind address");

    axum::serve(listener, app)
        .await
        .expect("Server error");
}
