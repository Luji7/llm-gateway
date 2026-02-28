mod config;
mod error;
mod handlers;
mod models;
mod metrics;
mod state;
mod tracing_otlp;
mod streaming;
mod translate;
mod audit_log;

use axum::{routing::post, Router};
use handlers::post_messages;
use metrics::{init_metrics, init_metrics_noop, MetricsExporterConfig};
use tracing_otlp::{init_tracer_grpc, init_tracer_langfuse_http, init_tracer_noop, spawn_tracer_watchdog};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::Config;
use crate::state::AppState;
use crate::audit_log::AuditLogger;
use std::fs::OpenOptions;
use std::path::Path;
use std::sync::Arc;

fn parse_level(level: &str) -> LevelFilter {
    match level {
        "trace" => LevelFilter::TRACE,
        "debug" => LevelFilter::DEBUG,
        "warn" => LevelFilter::WARN,
        "error" => LevelFilter::ERROR,
        _ => LevelFilter::INFO,
    }
}

fn open_log_file(path: &str) -> Option<std::fs::File> {
    let path = Path::new(path);
    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            eprintln!("log file create dir error: {}", err);
            return None;
        }
    }
    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(file) => Some(file),
        Err(err) => {
            eprintln!("log file open error: {}", err);
            None
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let config = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("config error: {}", err);
            std::process::exit(1);
        }
    };

    let inflight_count = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let metrics_exporter = MetricsExporterConfig {
        kind: config.observability.exporters.metrics.clone(),
        endpoint: if config.observability.exporters.metrics == "langfuse_http" {
            config.observability.otlp_http.metrics_endpoint()
        } else {
            config.observability.otlp_grpc.endpoint.clone()
        },
        timeout_ms: if config.observability.exporters.metrics == "langfuse_http" {
            config.observability.otlp_http.timeout_ms
        } else {
            config.observability.otlp_grpc.timeout_ms
        },
        public_key: config.observability.otlp_http.public_key.clone(),
        secret_key: config.observability.otlp_http.secret_key.clone(),
    };

    let metrics = match init_metrics(
        config.observability.service_name.clone(),
        metrics_exporter,
        inflight_count.clone(),
    ) {
        Ok(m) => m,
        Err(err) => {
            eprintln!("metrics init error (fallback to noop): {}", err);
            init_metrics_noop(inflight_count.clone())
        }
    };
    let tracer_provider = match config.observability.exporters.tracing.as_str() {
        "langfuse_http" => init_tracer_langfuse_http(
            config.observability.otlp_http.traces_endpoint(),
            config.observability.service_name.clone(),
            config.observability.otlp_http.timeout_ms,
            config.observability.otlp_http.public_key.clone(),
            config.observability.otlp_http.secret_key.clone(),
        ),
        _ => init_tracer_grpc(
            config.observability.otlp_grpc.endpoint.clone(),
            config.observability.service_name.clone(),
            config.observability.otlp_grpc.timeout_ms,
        ),
    };
    let tracer_provider = match tracer_provider {
        Ok(provider) => provider,
        Err(err) => {
            eprintln!("tracing init error (fallback to noop): {}", err);
            init_tracer_noop(config.observability.service_name.clone())
        }
    };

    let log_level = parse_level(config.observability.logging.level.as_str());
    let log_format = config.observability.logging.format.as_str();
    let file_writer = config
        .observability
        .logging
        .file
        .as_deref()
        .and_then(open_log_file)
        .map(Arc::new);

    let writer = match (config.observability.logging.stdout, file_writer) {
        (true, Some(file)) => BoxMakeWriter::new(std::io::stdout.and(file)),
        (true, None) => BoxMakeWriter::new(std::io::stdout),
        (false, Some(file)) => BoxMakeWriter::new(file),
        (false, None) => BoxMakeWriter::new(std::io::stdout),
    };

    if log_format == "json" {
        eprintln!("logging.format=json is not enabled; falling back to text");
    }
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_filter(log_level);

    let telemetry = tracing_opentelemetry::layer();
    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(telemetry)
        .init();

    let tracing_exporter_kind = config.observability.exporters.tracing.as_str();
    let tracing_endpoint = if tracing_exporter_kind == "langfuse_http" {
        config.observability.otlp_http.traces_endpoint()
    } else {
        config.observability.otlp_grpc.endpoint.clone()
    };
    tracing::info!(
        tracing_exporter = tracing_exporter_kind,
        tracing_endpoint = %tracing_endpoint,
        tracing_batch = true,
        "tracing exporter configured"
    );

    let _tracer_watchdog = spawn_tracer_watchdog(tracer_provider.clone());

    let state = AppState {
        client: reqwest::Client::builder()
            .pool_max_idle_per_host(config.downstream.pool_max_idle_per_host)
            .connect_timeout(config.connect_timeout())
            .timeout(config.read_timeout())
            .build()
            .unwrap_or_else(|e| {
                eprintln!("client build error: {}", e);
                std::process::exit(1);
            }),
        stream_client: reqwest::Client::builder()
            .pool_max_idle_per_host(config.downstream.pool_max_idle_per_host)
            .connect_timeout(config.connect_timeout())
            .build()
            .unwrap_or_else(|e| {
                eprintln!("stream client build error: {}", e);
                std::process::exit(1);
            }),
        config: config.clone(),
        inflight: std::sync::Arc::new(tokio::sync::Semaphore::new(config.limits.max_inflight)),
        inflight_count,
        metrics,
        audit_logger: if config.observability.audit_log.enabled {
            match config.observability.audit_log.path.as_deref() {
                Some(path) => AuditLogger::new(
                    path.to_string(),
                    config.observability.audit_log.max_file_bytes,
                )
                .ok(),
                None => None,
            }
        } else {
            None
        },
        _tracer_provider: tracer_provider,
    };

    let app = Router::new()
        .route("/v1/messages", post(post_messages))
        .route("/v1/models", axum::routing::get(handlers::get_models))
        .route("/health", axum::routing::get(handlers::health))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&config.server.bind_addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("bind error: {}", e);
            std::process::exit(1);
        });

    tracing::info!("listening on {}", config.server.bind_addr);
    axum::serve(listener, app).await.unwrap();
}
