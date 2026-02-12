# AI Gateway (Anthropic -> OpenAI Compat)

Rust 实现的轻量级 LLM 网关代理服务，接收 Anthropic Messages API 格式请求，转换为 OpenAI Chat Completions 兼容格式并转发下游。

## 运行

```bash
CONFIG_PATH=./config.yaml \
cargo run
```

## 交叉编译（Mac -> Ubuntu 22.04 x86_64，无 Docker）

1) 安装 Zig（Mac）

```bash
brew install zig
```

2) 添加 Rust 目标

```bash
rustup target add x86_64-unknown-linux-gnu
```

3) 创建 Zig 包装脚本（过滤 `--target=x86_64-unknown-linux-gnu`）

```bash
mkdir -p scripts
cat > scripts/zig-cc.sh <<'EOF'
#!/usr/bin/env bash
args=()
for arg in "$@"; do
  case "$arg" in
    --target=x86_64-unknown-linux-gnu) ;;
    --target) shift ;;
    *) args+=("$arg") ;;
  esac
done
exec zig cc -target x86_64-linux-gnu "${args[@]}"
EOF

cat > scripts/zig-ar.sh <<'EOF'
#!/usr/bin/env bash
exec zig ar "$@"
EOF

chmod +x scripts/zig-cc.sh scripts/zig-ar.sh
```

4) 编译

```bash
CC="$(pwd)/scripts/zig-cc.sh" \
AR="$(pwd)/scripts/zig-ar.sh" \
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="$(pwd)/scripts/zig-cc.sh" \
cargo build --release --target x86_64-unknown-linux-gnu
```

产物：

- `target/x86_64-unknown-linux-gnu/release/llm-gateway`

## 最小请求示例

```bash
curl -s http://localhost:8080/v1/messages \
  -H 'content-type: application/json' \
  -d '{
    "model": "kimi-k2.5",
    "system": "你是一个AI助手",
    "max_tokens": 256,
    "messages": [{"role":"user","content":"Hello"}]
  }'
```

## 流式工具调用示例

```bash
curl -N http://localhost:8080/v1/messages \
  -H 'content-type: application/json' \
  -d '{
    "model": "kimi-k2.5",
    "max_tokens": 128,
    "stream": true,
    "tools": [
      {
        "name": "get_weather",
        "description": "获取指定城市天气",
        "input_schema": {
          "type": "object",
          "properties": {
            "location": { "type": "string" }
          },
          "required": ["location"]
        }
      }
    ],
    "tool_choice": { "type": "tool", "name": "get_weather" },
    "messages": [
      { "role": "user", "content": "北京天气怎么样？" }
    ]
  }'
```

## 当前限制（Phase 2）

- document block 默认 `reject`（可通过 `DOCUMENT_POLICY` 调整）
- 多模态仅支持 image base64 -> data URL（`ALLOW_IMAGES` 控制）
- 流式 SSE 为最佳努力转换（tool_calls / reasoning 部分场景依赖下游实际返回）

## /v1/models 代理

- `GET /v1/models` 返回 Anthropic 规范结构
- 可通过 `MODELS_JSON` 直接指定返回列表（优先于下游）
- `MODEL_DISPLAY_MAP` 可覆盖 display_name

### /v1/models 示例

```bash
curl -s http://localhost:8080/v1/models
```

### /v1/models 自定义返回（YAML）

```bash
CONFIG_PATH=./config.yaml cargo run
```

```yaml
models:
  models_override:
    - id: "kimi-k2.5"
      type: "model"
      display_name: "Kimi K2.5"
      created_at: "2025-01-01T00:00:00Z"
    - id: "gpt-4o-mini"
      type: "model"
      display_name: "GPT-4o Mini"
      created_at: "2024-08-01T00:00:00Z"
```

## 配置（YAML，严格模式）

仅支持通过 `CONFIG_PATH` 指定配置文件路径，环境变量不再覆盖配置内容：

```bash
CONFIG_PATH=./config.yaml cargo run
```

示例 `config.yaml`（按分组组织）：

```yaml
server:
  bind_addr: "0.0.0.0:8080"

downstream:
  base_url: "https://api.moonshot.cn/v1"
  api_key: "sk-xxx"
  connect_timeout_ms: 5000
  read_timeout_ms: 60000
  pool_max_idle_per_host: 64

models:
  model_map:
    kimi-k2.5: kimi-k2.5
  display_map:
    gpt-4o-mini: "GPT-4o Mini"
  allowlist: []
  blocklist: []
  thinking_map:
    4000: "medium"
    8000: "high"
  output_strict: true
  allow_images: true
  document_policy: "reject"
  models_override: null

limits:
  max_inflight: 512

observability:
  service_name: "llm-gateway"
  dump_downstream: false
  logging:
    level: "info"
    format: "text" # "json" 将回退为 text（未启用 json feature）
    stdout: true
    file: "./logs/llm-gateway.log"
  otlp_grpc:
    endpoint: "http://localhost:4317"
    timeout_ms: 3000
  otlp_http:
    base_url: "https://cloud.langfuse.com/api/public/otel"
    public_key: "pk_***"
    secret_key: "sk_***"
    timeout_ms: 5000
  exporters:
    tracing: "langfuse_http" # or "otlp_grpc"
    metrics: "langfuse_http" # or "otlp_grpc"
```

## Langfuse OTLP（HTTP）

使用 Langfuse 时建议将 tracing/metrics 的 exporter 改为 `langfuse_http`，并提供 public/secret key：

```yaml
observability:
  otlp_http:
    base_url: "https://cloud.langfuse.com/api/public/otel"
    public_key: "pk_***"
    secret_key: "sk_***"
    timeout_ms: 5000
  exporters:
    tracing: "langfuse_http"
    metrics: "langfuse_http"
```

说明：
- Langfuse 使用 HTTP OTLP，网关会自动用 Basic Auth 头（public:secret）推送

## 日志输出

- 默认输出到 stdout
- 配置 `observability.logging.file` 可同时写入日志文件
- `format: json` 当前会回退为文本（未启用 json feature）

示例（同时输出 stdout + 文件）：

```yaml
observability:
  logging:
    level: "info"
    format: "text"
    stdout: true
    file: "./logs/llm-gateway.log"
```

## Trace 说明

- Trace span 会记录 `downstream.request` 与 `downstream.response`（流式为拼接的 `data:` 内容）
- 体积由 `TRACE_BODY_MAX_BYTES` 限制，超过会截断

示例（Langfuse Trace 中的属性）：

```text
input=[{"role":"system","content":"你是一个AI助手"},{"role":"user","content":"Hello"}]
output=[{"role":"assistant","content":"Hello! How can I help you today?"}]
downstream.request={"model":"kimi-k2.5","messages":[...],"max_tokens":256,...}
downstream.response={"id":"chatcmpl-...","choices":[...],...}
```

## OTLP 失败降级

- tracing 或 metrics 初始化失败时，会自动降级为 noop（不阻塞服务启动）

## 目录结构

- `src/main.rs`: 入口与路由
- `src/config.rs`: 配置加载
- `src/models.rs`: 请求/响应结构体
- `src/translate.rs`: 转换逻辑
- `src/handlers.rs`: HTTP handler
