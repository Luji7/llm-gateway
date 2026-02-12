use opentelemetry::metrics::{Counter, Histogram, ObservableGauge};
use opentelemetry::metrics::MeterProvider;
use opentelemetry_otlp::{MetricExporter, Protocol, WithExportConfig, WithHttpConfig};
use std::time::Duration;
use std::collections::HashMap;
use base64::Engine;
use opentelemetry_sdk::metrics::periodic_reader_with_async_runtime::PeriodicReader;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::runtime;
use opentelemetry_sdk::Resource;
use std::sync::{atomic::AtomicU64, Arc};

#[derive(Clone)]
pub struct Metrics {
    pub requests: Counter<u64>,
    pub errors: Counter<u64>,
    pub latency_ms: Histogram<f64>,
    _inflight: ObservableGauge<i64>,
}

pub fn init_metrics(
    service_name: String,
    exporter: MetricsExporterConfig,
    inflight_count: Arc<AtomicU64>,
) -> Result<Metrics, String> {
    let exporter = match exporter.kind.as_str() {
        "langfuse_http" => {
            let auth = base64::engine::general_purpose::STANDARD.encode(format!(
                "{}:{}",
                exporter.public_key, exporter.secret_key
            ));
            let headers = HashMap::from([(String::from("Authorization"), format!("Basic {}", auth))]);
            MetricExporter::builder()
                .with_http()
                .with_endpoint(exporter.endpoint)
                .with_protocol(Protocol::HttpBinary)
                .with_timeout(Duration::from_millis(exporter.timeout_ms))
                .with_headers(headers)
                .build()
                .map_err(|e| format!("metrics exporter init error: {}", e))?
        }
        _ => MetricExporter::builder()
            .with_tonic()
            .with_endpoint(exporter.endpoint)
            .with_protocol(Protocol::Grpc)
            .with_timeout(Duration::from_millis(exporter.timeout_ms))
            .build()
            .map_err(|e| format!("metrics exporter init error: {}", e))?,
    };

    let reader = PeriodicReader::builder(exporter, runtime::Tokio).build();
    let provider = SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(Resource::builder().with_service_name(service_name).build())
        .build();

    let meter = provider.meter("llm-gateway");
    opentelemetry::global::set_meter_provider(provider);

    let requests = meter
        .u64_counter("ai.gateway.requests")
        .with_description("Total requests")
        .build();
    let errors = meter
        .u64_counter("ai.gateway.errors")
        .with_description("Total errors")
        .build();
    let latency_ms = meter
        .f64_histogram("ai.gateway.latency_ms")
        .with_unit("ms")
        .with_description("Request latency in ms")
        .build();
    let inflight = meter
        .i64_observable_gauge("ai.gateway.inflight")
        .with_description("In-flight requests")
        .with_callback(move |observer| {
            let value = inflight_count.load(std::sync::atomic::Ordering::Relaxed) as i64;
            observer.observe(value, &[]);
        })
        .build();

    Ok(Metrics {
        requests,
        errors,
        latency_ms,
        _inflight: inflight,
    })
}

pub fn init_metrics_noop(inflight_count: Arc<AtomicU64>) -> Metrics {
    let meter = opentelemetry::global::meter("llm-gateway");
    let requests = meter.u64_counter("ai.gateway.requests").build();
    let errors = meter.u64_counter("ai.gateway.errors").build();
    let latency_ms = meter.f64_histogram("ai.gateway.latency_ms").build();
    let inflight = meter
        .i64_observable_gauge("ai.gateway.inflight")
        .with_callback(move |observer| {
            let value = inflight_count.load(std::sync::atomic::Ordering::Relaxed) as i64;
            observer.observe(value, &[]);
        })
        .build();

    Metrics {
        requests,
        errors,
        latency_ms,
        _inflight: inflight,
    }
}

pub struct MetricsExporterConfig {
    pub kind: String,
    pub endpoint: String,
    pub timeout_ms: u64,
    pub public_key: String,
    pub secret_key: String,
}
