use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use serde::Serialize;

const MAX_LOG_ENTRIES: usize = 500;
const MAX_METRIC_HISTORY: usize = 300; // ~15 minutes at 3s intervals

#[derive(Debug, Clone, Serialize)]
pub struct PerfMetrics {
    pub timestamp: f64, // seconds since start
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub skipped: u64,
    pub gpu_copy_ms: f64,
    pub map_ms: f64,
    pub readback_ms: f64,
    pub encode_ms: f64,
    pub total_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub timestamp: f64, // seconds since start
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DebugSnapshot {
    pub latest: Option<PerfMetrics>,
    pub history: Vec<PerfMetrics>,
    pub logs: Vec<LogEntry>,
    pub uptime_secs: f64,
}

pub struct DebugStoreInner {
    start_time: std::time::Instant,
    metrics_history: VecDeque<PerfMetrics>,
    logs: VecDeque<LogEntry>,
}

#[derive(Clone)]
pub struct DebugStore(Arc<Mutex<DebugStoreInner>>);

impl DebugStore {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(DebugStoreInner {
            start_time: std::time::Instant::now(),
            metrics_history: VecDeque::with_capacity(MAX_METRIC_HISTORY),
            logs: VecDeque::with_capacity(MAX_LOG_ENTRIES),
        })))
    }

    pub fn push_metrics(&self, width: u32, height: u32, fps: f64, skipped: u64, gpu_copy_ms: f64, map_ms: f64, readback_ms: f64, encode_ms: f64, total_ms: f64) {
        let mut inner = self.0.lock().unwrap();
        let timestamp = inner.start_time.elapsed().as_secs_f64();
        let m = PerfMetrics {
            timestamp,
            width,
            height,
            fps,
            skipped,
            gpu_copy_ms,
            map_ms,
            readback_ms,
            encode_ms,
            total_ms,
        };
        if inner.metrics_history.len() >= MAX_METRIC_HISTORY {
            inner.metrics_history.pop_front();
        }
        inner.metrics_history.push_back(m);
    }

    pub fn push_log(&self, message: String) {
        let mut inner = self.0.lock().unwrap();
        let timestamp = inner.start_time.elapsed().as_secs_f64();
        if inner.logs.len() >= MAX_LOG_ENTRIES {
            inner.logs.pop_front();
        }
        inner.logs.push_back(LogEntry { timestamp, message });
    }

    pub fn snapshot(&self) -> DebugSnapshot {
        let inner = self.0.lock().unwrap();
        DebugSnapshot {
            latest: inner.metrics_history.back().cloned(),
            history: inner.metrics_history.iter().cloned().collect(),
            logs: inner.logs.iter().cloned().collect(),
            uptime_secs: inner.start_time.elapsed().as_secs_f64(),
        }
    }
}
