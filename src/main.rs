mod capture;
mod config;
mod debug;
mod server;
mod stream;

use std::sync::Arc;
use tokio::sync::{watch, RwLock};

#[tokio::main]
async fn main() {
    loop {
        if !run_server().await {
            break;
        }
        eprintln!("Restarting server...");
    }
}

/// Run the server. Returns `true` if a restart was requested.
async fn run_server() -> bool {
    let cfg = config::load_config();
    let host = cfg.server.host.clone();
    let port = cfg.server.port;

    let shared_config: config::SharedConfig = Arc::new(RwLock::new(cfg));

    let debug_store = debug::DebugStore::new();
    let restart_signal = Arc::new(tokio::sync::Notify::new());

    // watch channel: initial empty frame
    let (frame_tx, frame_rx) = watch::channel(Arc::new(Vec::<u8>::new()));

    // Start capture thread
    let capture_config = shared_config.clone();
    let capture_debug = debug_store.clone();
    let _capture_handle = capture::start_capture_thread(frame_tx, capture_config, capture_debug);

    debug_store.push_log(format!("Server starting on http://{host}:{port}"));

    // Build HTTP server
    let state = server::AppState {
        config: shared_config,
        frame_rx,
        debug: debug_store,
        restart_signal: restart_signal.clone(),
    };
    let app = server::create_router(state);

    let addr = format!("{host}:{port}");
    eprintln!("Listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Failed to bind address");

    let signal = restart_signal.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            signal.notified().await;
        })
        .await
        .expect("Server error");

    true
}
