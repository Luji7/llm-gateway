use opentelemetry::global;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::trace::span_processor_with_async_runtime::BatchSpanProcessor;
use opentelemetry_sdk::runtime;
use std::sync::OnceLock;
use tracing::warn;
use opentelemetry_otlp::{SpanExporter, WithExportConfig, WithHttpConfig, Protocol};
use std::collections::HashMap;
use std::time::Duration;
use base64::Engine;

pub fn init_tracer_grpc(
    otlp_endpoint: String,
    service_name: String,
    otlp_timeout_ms: u64,
) -> Result<SdkTracerProvider, String> {
    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(otlp_endpoint)
        .with_timeout(Duration::from_millis(otlp_timeout_ms))
        .build()
        .map_err(|e| format!("trace exporter init error: {}", e))?;

    let batch = BatchSpanProcessor::builder(exporter, runtime::Tokio).build();
    let provider = SdkTracerProvider::builder()
        .with_span_processor(batch)
        .with_resource(Resource::builder().with_service_name(service_name).build())
        .build();

    hold_tracer_provider(provider.clone());
    Ok(provider)
}

pub fn init_tracer_langfuse_http(
    endpoint: String,
    service_name: String,
    timeout_ms: u64,
    public_key: String,
    secret_key: String,
) -> Result<SdkTracerProvider, String> {
    let auth = base64::engine::general_purpose::STANDARD.encode(format!(
        "{}:{}",
        public_key, secret_key
    ));
    let headers = HashMap::from([(String::from("Authorization"), format!("Basic {}", auth))]);

    let exporter = SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .with_protocol(Protocol::HttpBinary)
        .with_timeout(Duration::from_millis(timeout_ms))
        .with_headers(headers)
        .build()
        .map_err(|e| format!("langfuse tracer init error: {}", e))?;

    let batch = BatchSpanProcessor::builder(exporter, runtime::Tokio).build();
    let provider = SdkTracerProvider::builder()
        .with_span_processor(batch)
        .with_resource(Resource::builder().with_service_name(service_name).build())
        .build();

    hold_tracer_provider(provider.clone());
    Ok(provider)
}

pub fn init_tracer_noop(service_name: String) -> SdkTracerProvider {
    let provider = SdkTracerProvider::builder()
        .with_resource(Resource::builder().with_service_name(service_name).build())
        .build();
    hold_tracer_provider(provider.clone());
    provider
}

fn hold_tracer_provider(provider: SdkTracerProvider) {
    static GLOBAL_PROVIDER: OnceLock<SdkTracerProvider> = OnceLock::new();
    let _ = GLOBAL_PROVIDER.set(provider.clone());
    global::set_tracer_provider(provider);
}

pub fn spawn_tracer_watchdog(provider: SdkTracerProvider) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(30));
        if let Err(err) = provider.force_flush() {
            warn!(
                "tracer provider force_flush failed (batch worker may be down): {}",
                err
            );
        }
    })
}
