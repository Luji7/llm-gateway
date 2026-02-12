use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::Duration;

use crate::models::AnthropicModel;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub downstream: DownstreamConfig,
    pub models: ModelsConfig,
    pub limits: LimitsConfig,
    pub observability: ObservabilityConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_bind_addr")]
    pub bind_addr: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DownstreamConfig {
    #[serde(default = "default_openai_base_url")]
    pub base_url: String,
    pub api_key: String,
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    #[serde(default = "default_read_timeout_ms")]
    pub read_timeout_ms: u64,
    #[serde(default = "default_pool_max_idle_per_host")]
    pub pool_max_idle_per_host: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModelsConfig {
    #[serde(default)]
    pub model_map: HashMap<String, String>,
    #[serde(default)]
    pub display_map: HashMap<String, String>,
    #[serde(default)]
    pub allowlist: HashSet<String>,
    #[serde(default)]
    pub blocklist: HashSet<String>,
    #[serde(default)]
    pub thinking_map: HashMap<u32, String>,
    #[serde(default = "default_output_strict")]
    pub output_strict: bool,
    #[serde(default = "default_allow_images")]
    pub allow_images: bool,
    #[serde(default = "default_document_policy")]
    pub document_policy: String,
    #[serde(default)]
    pub models_override: Option<Vec<AnthropicModel>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LimitsConfig {
    #[serde(default = "default_max_inflight")]
    pub max_inflight: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ObservabilityConfig {
    #[serde(default = "default_service_name")]
    pub service_name: String,
    #[serde(default)]
    pub dump_downstream: bool,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub otlp_grpc: OtlpGrpcConfig,
    #[serde(default)]
    pub otlp_http: OtlpHttpConfig,
    #[serde(default)]
    pub exporters: ExportersConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct OtlpGrpcConfig {
    #[serde(default = "default_otlp_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_otlp_timeout_ms")]
    pub timeout_ms: u64,
}

impl Default for OtlpGrpcConfig {
    fn default() -> Self {
        Self {
            endpoint: default_otlp_endpoint(),
            timeout_ms: default_otlp_timeout_ms(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct OtlpHttpConfig {
    #[serde(default = "default_langfuse_http_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub public_key: String,
    #[serde(default)]
    pub secret_key: String,
    #[serde(default = "default_langfuse_timeout_ms")]
    pub timeout_ms: u64,
}

impl Default for OtlpHttpConfig {
    fn default() -> Self {
        Self {
            base_url: default_langfuse_http_base_url(),
            public_key: String::new(),
            secret_key: String::new(),
            timeout_ms: default_langfuse_timeout_ms(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ExportersConfig {
    #[serde(default = "default_exporter_kind")]
    pub tracing: String,
    #[serde(default = "default_exporter_kind")]
    pub metrics: String,
}

impl Default for ExportersConfig {
    fn default() -> Self {
        Self {
            tracing: default_exporter_kind(),
            metrics: default_exporter_kind(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_format")]
    pub format: String,
    #[serde(default = "default_log_stdout")]
    pub stdout: bool,
    #[serde(default)]
    pub file: Option<String>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
            stdout: default_log_stdout(),
            file: None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum DocumentPolicy {
    Reject,
    Strip,
    TextOnly,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let path = std::env::var("CONFIG_PATH")
            .map_err(|_| "CONFIG_PATH is required (strict YAML)".to_string())?;
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("CONFIG_PATH read error: {}", e))?;
        let mut config: Config = serde_yaml::from_str(&content)
            .map_err(|e| format!("CONFIG_PATH invalid yaml: {}", e))?;
        config.normalize()?;
        Ok(config)
    }

    pub fn chat_completions_url(&self) -> String {
        let base = self.downstream.base_url.trim_end_matches('/');
        if base.ends_with("/v1") {
            format!("{}/chat/completions", base)
        } else {
            format!("{}/v1/chat/completions", base)
        }
    }

    pub fn models_url(&self) -> String {
        let base = self.downstream.base_url.trim_end_matches('/');
        if base.ends_with("/v1") {
            format!("{}/models", base)
        } else {
            format!("{}/v1/models", base)
        }
    }

    pub fn document_policy(&self) -> Result<DocumentPolicy, String> {
        match self.models.document_policy.as_str() {
            "reject" => Ok(DocumentPolicy::Reject),
            "strip" => Ok(DocumentPolicy::Strip),
            "text_only" => Ok(DocumentPolicy::TextOnly),
            other => Err(format!("DOCUMENT_POLICY invalid: {}", other)),
        }
    }

    pub fn thinking_map_pairs(&self) -> Vec<(u32, String)> {
        let mut entries: Vec<(u32, String)> = self
            .models
            .thinking_map
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        entries.sort_by_key(|(k, _)| *k);
        entries
    }

    pub fn connect_timeout(&self) -> Duration {
        Duration::from_millis(self.downstream.connect_timeout_ms)
    }

    pub fn read_timeout(&self) -> Duration {
        Duration::from_millis(self.downstream.read_timeout_ms)
    }

    fn normalize(&mut self) -> Result<(), String> {
        if self.downstream.api_key.trim().is_empty() {
            return Err("downstream.api_key is required".to_string());
        }
        self.observability.logging.format =
            self.observability.logging.format.to_lowercase();
        self.observability.logging.level =
            self.observability.logging.level.to_lowercase();
        match self.observability.logging.format.as_str() {
            "text" | "json" => {}
            other => return Err(format!("logging.format invalid: {}", other)),
        }
        match self.observability.logging.level.as_str() {
            "trace" | "debug" | "info" | "warn" | "error" => {}
            other => return Err(format!("logging.level invalid: {}", other)),
        }
        Ok(())
    }
}

fn default_bind_addr() -> String {
    "0.0.0.0:8080".to_string()
}

fn default_openai_base_url() -> String {
    "https://api.openai.com".to_string()
}

fn default_connect_timeout_ms() -> u64 {
    5000
}

fn default_read_timeout_ms() -> u64 {
    60000
}

fn default_pool_max_idle_per_host() -> usize {
    64
}

fn default_max_inflight() -> usize {
    512
}

fn default_otlp_endpoint() -> String {
    "http://localhost:4317".to_string()
}

fn default_service_name() -> String {
    "llm-gateway".to_string()
}


fn default_otlp_timeout_ms() -> u64 {
    3000
}

fn default_tracing_exporter() -> String {
    "otlp_grpc".to_string()
}

fn default_langfuse_http_base_url() -> String {
    "https://cloud.langfuse.com/api/public/otel".to_string()
}

fn default_langfuse_timeout_ms() -> u64 {
    5000
}

fn default_langfuse_metrics_endpoint() -> String {
    "https://cloud.langfuse.com/api/public/otel/v1/metrics".to_string()
}

fn default_exporter_kind() -> String {
    "otlp_grpc".to_string()
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "text".to_string()
}

fn default_log_stdout() -> bool {
    true
}

impl OtlpHttpConfig {
    pub fn traces_endpoint(&self) -> String {
        format!("{}/v1/traces", self.base_url.trim_end_matches('/'))
    }

    pub fn metrics_endpoint(&self) -> String {
        format!("{}/v1/metrics", self.base_url.trim_end_matches('/'))
    }
}

fn default_allow_images() -> bool {
    true
}

fn default_document_policy() -> String {
    "reject".to_string()
}

fn default_output_strict() -> bool {
    true
}
