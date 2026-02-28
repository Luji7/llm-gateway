use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct AuditLogger {
    sender: mpsc::Sender<AuditLogRecord>,
}

impl AuditLogger {
    pub fn new(base_path: String, max_file_bytes: u64) -> Result<Self, String> {
        let (tx, mut rx) = mpsc::channel::<AuditLogRecord>(256);
        tokio::spawn(async move {
            let mut current_path = build_log_path(&base_path);
            let mut file = match open_log_file(&current_path).await {
                Ok(file) => file,
                Err(err) => {
                    tracing::error!("audit log open error: {}", err);
                    return;
                }
            };
            let mut current_size = file
                .metadata()
                .await
                .map(|m| m.len())
                .unwrap_or(0);
            while let Some(record) = rx.recv().await {
                if let Ok(line) = serde_json::to_string(&record) {
                    let projected = current_size + line.len() as u64 + 1;
                    if projected > max_file_bytes {
                        current_path = build_log_path(&base_path);
                        match open_log_file(&current_path).await {
                            Ok(new_file) => {
                                file = new_file;
                                current_size = 0;
                            }
                            Err(err) => {
                                tracing::error!("audit log rotate error: {}", err);
                            }
                        }
                    }
                    if file.write_all(line.as_bytes()).await.is_err() {
                        tracing::error!("audit log write error");
                        continue;
                    }
                    if file.write_all(b"\n").await.is_err() {
                        tracing::error!("audit log write error");
                    }
                    current_size += line.len() as u64 + 1;
                }
            }
        });
        Ok(Self { sender: tx })
    }

    pub async fn push(&self, record: AuditLogRecord) {
        let _ = self.sender.send(record).await;
    }
}

#[derive(Clone)]
pub struct AuditContext {
    pub ts_start_ms: u128,
    pub request_id: String,
    pub route: String,
    pub mode: String,
    pub method: String,
    pub request_headers: HashMap<String, String>,
    pub request_body: Value,
    pub meta: AuditMeta,
}

impl AuditContext {
    pub fn finish(
        self,
        status: u16,
        response_headers: HashMap<String, String>,
        response_body: Value,
        body_parse_error: bool,
        body_truncated: bool,
        ts_end_ms: u128,
    ) -> AuditLogRecord {
        AuditLogRecord {
            ts_start_ms: self.ts_start_ms,
            ts_end_ms,
            request_id: self.request_id,
            route: self.route,
            mode: self.mode,
            method: self.method,
            request: AuditMessage {
                headers: self.request_headers,
                body: self.request_body,
            },
            response: AuditResponse {
                status,
                headers: response_headers,
                body: response_body,
            },
            meta: AuditMeta {
                model: self.meta.model,
                stream: self.meta.stream,
                body_truncated,
                body_parse_error,
            },
        }
    }
}

#[derive(Clone, Serialize)]
pub struct AuditLogRecord {
    pub ts_start_ms: u128,
    pub ts_end_ms: u128,
    pub request_id: String,
    pub route: String,
    pub mode: String,
    pub method: String,
    pub request: AuditMessage,
    pub response: AuditResponse,
    pub meta: AuditMeta,
}

#[derive(Clone, Serialize)]
pub struct AuditMessage {
    pub headers: HashMap<String, String>,
    pub body: Value,
}

#[derive(Clone, Serialize)]
pub struct AuditResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Value,
}

#[derive(Clone, Serialize)]
pub struct AuditMeta {
    pub model: Option<String>,
    pub stream: Option<bool>,
    pub body_truncated: bool,
    pub body_parse_error: bool,
}

pub fn headers_to_map(headers: &axum::http::HeaderMap) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (name, value) in headers.iter() {
        let value = value.to_str().unwrap_or("[invalid]");
        if name.as_str().eq_ignore_ascii_case("authorization") {
            out.insert(name.to_string(), "[redacted]".to_string());
        } else {
            out.insert(name.to_string(), value.to_string());
        }
    }
    out
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn build_log_path(base: &str) -> String {
    let ts = now_ms();
    if let Some(stripped) = base.strip_suffix(".jsonl") {
        format!("{}.{}.jsonl", stripped, ts)
    } else {
        format!("{}.{}", base, ts)
    }
}

async fn open_log_file(path: &str) -> Result<tokio::fs::File, std::io::Error> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
}
