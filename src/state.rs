use crate::config::Config;
use crate::audit_log::AuditLogger;
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use crate::metrics::Metrics;

#[derive(Clone)]
pub struct AppState {
    pub client: reqwest::Client,
    pub stream_client: reqwest::Client,
    pub config: Config,
    pub inflight: Arc<Semaphore>,
    pub inflight_count: Arc<AtomicU64>,
    pub metrics: Metrics,
    pub audit_logger: Option<AuditLogger>,
    pub _tracer_provider: opentelemetry_sdk::trace::SdkTracerProvider,
}

pub struct InflightGuard {
    _permit: OwnedSemaphorePermit,
    counter: Arc<AtomicU64>,
}

impl InflightGuard {
    pub fn new(permit: OwnedSemaphorePermit, counter: Arc<AtomicU64>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self { _permit: permit, counter }
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}
