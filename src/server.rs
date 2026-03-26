use std::sync::Arc;
use axum::{
    Router,
    extract::State,
    extract::ws::WebSocketUpgrade,
    response::{Html, IntoResponse},
    routing::get,
    Json,
};
use axum::http::StatusCode;
use tokio::sync::watch;

use crate::capture;
use crate::config::{self, SharedConfig};
use crate::debug::DebugStore;
use crate::stream;

#[derive(Clone)]
pub struct AppState {
    pub config: SharedConfig,
    pub frame_rx: watch::Receiver<Arc<Vec<u8>>>,
    pub debug: DebugStore,
    pub restart_signal: Arc<tokio::sync::Notify>,
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/raw", get(raw_handler))
        .route("/ws", get(ws_handler))
        .route("/config", get(config_page_handler))
        .route("/debug", get(debug_page_handler))
        .route("/api/config", get(get_config_handler).post(post_config_handler))
        .route("/api/windows", get(windows_handler))
        .route("/api/debug", get(debug_handler))
        .route("/api/health", get(health_handler))
        .route("/api/ping", get(ping_handler))
        .with_state(state)
}

async fn index_handler() -> Html<&'static str> {
    Html(include_str!("index.html"))
}

async fn config_page_handler() -> Html<&'static str> {
    Html(include_str!("config_page.html"))
}

async fn raw_handler() -> Html<&'static str> {
    Html(include_str!("raw_page.html"))
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    let fps = {
        let cfg = state.config.read().await;
        cfg.capture.target_fps
    };
    ws.on_upgrade(move |socket| stream::ws_stream(socket, state.frame_rx.clone(), fps))
}

async fn get_config_handler(State(state): State<AppState>) -> impl IntoResponse {
    let cfg = state.config.read().await;
    Json(cfg.clone())
}

async fn post_config_handler(
    State(state): State<AppState>,
    Json(new_config): Json<config::Config>,
) -> impl IntoResponse {
    // Validate
    if new_config.capture.target_fps == 0 || new_config.capture.target_fps > 120 {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
            "error": "target_fps must be between 1 and 120"
        }))).into_response();
    }
    if new_config.capture.quality == 0 || new_config.capture.quality > 100 {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
            "error": "quality must be between 1 and 100"
        }))).into_response();
    }

    // Check if restart is needed (target_fps changed)
    let needs_restart = {
        let cfg = state.config.read().await;
        cfg.capture.target_fps != new_config.capture.target_fps
    };

    // Save to file
    if let Err(e) = config::save_config_to_file(&new_config) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
            "error": e
        }))).into_response();
    }

    // Update shared state
    {
        let mut cfg = state.config.write().await;
        *cfg = new_config.clone();
    }

    if needs_restart {
        state.debug.push_log("target_fps changed, restarting server...".to_string());
        let signal = state.restart_signal.clone();
        // Signal restart after a short delay to allow the response to be sent
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            signal.notify_one();
        });
    }

    Json(serde_json::json!({
        "config": new_config,
        "restarting": needs_restart,
    })).into_response()
}

async fn windows_handler() -> impl IntoResponse {
    let windows = capture::enum_windows();
    Json(windows)
}

async fn debug_page_handler() -> Html<&'static str> {
    Html(include_str!("debug_page.html"))
}

async fn debug_handler(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.debug.snapshot())
}

async fn ping_handler() -> impl IntoResponse {
    Json(serde_json::json!({ "pong": true }))
}

async fn health_handler(State(state): State<AppState>) -> impl IntoResponse {
    let snapshot = state.debug.snapshot();
    let target_fps = {
        let cfg = state.config.read().await;
        cfg.capture.target_fps
    };
    let capturing = snapshot.latest.as_ref().map_or(false, |m| {
        snapshot.uptime_secs - m.timestamp < 10.0
    });
    let client_connected = stream::is_stream_active();
    let fps = snapshot.latest.as_ref().map(|m| m.fps);
    Json(serde_json::json!({
        "capturing": capturing,
        "client_connected": client_connected,
        "fps": fps,
        "target_fps": target_fps,
    }))
}
