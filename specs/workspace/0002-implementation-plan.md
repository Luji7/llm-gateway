# 0002 - AI 网关代理服务实现计划

**文档状态**: Draft
**版本**: v0.1
**依据**: `specs/workspace/0001-spec.md`
**目标**: 产出可工作的 Anthropic -> OpenAI 兼容代理服务（Phase 1）并为后续 Phase 2/3 打基础

---

## 0. 当前实现情况（截至 2026-02-01）

### 已完成
- Rust 服务骨架与 `/v1/messages` 入口
- Phase 1 text-only 双向转换（Anthropic -> OpenAI，OpenAI -> Anthropic）
- Phase 2 功能落地：tools / output_format / thinking / image / streaming
- 基础错误映射与下游错误包装
- 配置项：`OPENAI_BASE_URL`（自动兼容是否包含 `/v1`）、`OPENAI_API_KEY`、`BIND_ADDR`
- 模型名映射：`MODEL_MAP`（JSON 对象字符串）
- 调试日志开关：`DUMP_DOWNSTREAM=1` 打印下游原始响应
- Phase 2 配置项：`OUTPUT_STRICT` / `ALLOW_IMAGES` / `DOCUMENT_POLICY` / `THINKING_MAP`
- 流式 SSE 转换与 tool_calls 分片拼接
- 流式 tool_use 参数完整 JSON 校验（不合法返回 invalid_request_error）
- 单元测试覆盖：转换映射、边界用例、流式 text/tool_use 及错误路径

### 未完成（按计划进入 Phase 3）
- 可观测性（metrics/trace/log 规范化）
- 性能优化（连接池/并发/超时）
- 配置增强（动态加载/多模型路由）

---

## 1. 实现范围划分（Phased）

### Phase 1（MVP，优先完成）
- 非流式请求/响应转换
- 基础字段映射（model/messages/system/max_tokens/temperature/top_p/stop/tools/tool_choice）
- 仅支持 text 内容（忽略/拒绝 image/document/tool*）
- 基础错误转换
- 基础配置（OpenAI Base URL / API Key / 端口）
- 最小可运行服务 + 简单测试

### Phase 2（功能增强）
- SSE 流式转换
- 工具调用（tool_use/tool_result <-> tool_calls）
- 结构化输出映射（output_format <-> response_format）
- image block 支持（base64 -> data URL）
- reasoning_content / thinking 映射策略

### Phase 3（生产化）
- 更完善的错误映射与重试策略
- 监控/日志/trace
- 配置热加载/多模型映射
- 负载/性能优化

---

## 2. Phase 1 详细实施步骤

### 2.1 项目初始化
- 新建 Rust workspace 或单 crate（建议 `axum` + `reqwest` + `serde`）
- 目录建议：
  - `src/main.rs`：启动入口
  - `src/config.rs`：配置加载
  - `src/models/`：请求/响应结构体
  - `src/translate.rs`：转换逻辑
  - `src/handlers.rs`：HTTP handler

### 2.2 依赖选型
- `axum`：HTTP server
- `tokio`：异步运行时
- `serde` / `serde_json`：JSON 序列化
- `reqwest`：HTTP client
- `thiserror` / `anyhow`：错误处理
- `tracing` / `tracing-subscriber`：日志

### 2.3 数据结构定义（Phase 1 子集）
- Anthropic 请求结构：
  - `model`, `max_tokens`, `messages`, `system?`, `temperature?`, `top_p?`, `stop_sequences?`, `stream?`
  - `messages.content` 仅支持 string 或 `{type: "text"}`
- OpenAI 请求结构：
  - `model`, `messages`, `max_completion_tokens`, `temperature?`, `top_p?`, `stop?`, `stream?`

### 2.4 转换逻辑（Phase 1）
- `system` -> OpenAI `messages[0]` role=system
- `messages` role 直接映射（user/assistant）
- content 仅支持 text：
  - string -> string
  - array: 仅允许 `type=text`，否则返回 400
- `max_tokens` -> `max_completion_tokens`
- `stop_sequences` -> `stop`
- `stream` 只允许 false（Phase 1 不支持流式）
 - 模型名映射（`MODEL_MAP`）在转换前生效

### 2.5 HTTP Handler
- `POST /v1/messages`
- 请求校验：必填字段、role 合法、content 仅 text
- 组装 OpenAI 请求，调用下游
- 转换 OpenAI 响应为 Anthropic 格式
- 转换错误响应
 - 可选：`DUMP_DOWNSTREAM=1` 输出下游原始 JSON

### 2.6 错误处理
- 下游 4xx/5xx -> Anthropic error
- 本地校验错误 -> Anthropic invalid_request_error

### 2.7 测试
- 单元测试：
  - anthro->openai 映射
  - openai->anthro 映射
- 简单集成测试：使用 mock server 或 json fixture

---

## 3. Phase 1 交付清单

- 可运行 Rust 服务（`cargo run`）
- `POST /v1/messages` 可用
- 能转发到 OpenAI 兼容后端
- JSON 结构完整映射（text-only）
- 基础错误响应
- 基本测试（含边界用例）
- 额外配置：`MODEL_MAP`、`DUMP_DOWNSTREAM`

---

## 4. 后续 Phase 重点

- Phase 2：流式 SSE / 工具调用 / 多模态
- Phase 3：可观测性、性能与配置增强

---

## 5. Phase 2 详细设计与实施计划

### 5.1 目标与范围
- 支持 `stream=true` 的 Anthropic SSE 输出
- 支持工具调用的请求与响应映射
- 支持结构化输出（JSON Schema）映射
- 支持 image block（base64 -> data URL）
- 支持 reasoning_content <-> thinking 的最小映射策略

### 5.2 接口与协议扩展
- 请求：接受 `tools` / `tool_choice` / `output_format` / `thinking`
- 响应：可能返回 `tool_use` / `thinking` / `redacted_thinking`
- 流式：将 OpenAI `chat.completion.chunk` 转换为 Anthropic SSE 事件序列

### 5.3 数据结构扩展（Rust）
新增或扩展结构体（建议新增文件 `src/models_openai.rs` / `src/models_anthropic.rs` 或继续在 `src/models.rs` 内扩展）：
- Anthropic
  - `AnthropicContentBlock` 扩展：`image`, `document`, `tool_result`, `tool_use`, `thinking`, `redacted_thinking`
  - `AnthropicTool`, `AnthropicToolChoice`, `AnthropicOutputFormat`
- OpenAI
  - `OpenAIMessage.content`: 支持多模态数组（text/image_url/file）
  - `OpenAITool`, `OpenAIToolChoice`
  - `OpenAIResponseFormat`（json_schema）
  - `OpenAIToolCall`, `OpenAIToolCallFunction`
  - `OpenAIStreamChunk`（SSE chunk）

### 5.4 请求映射细则（Anthropic -> OpenAI）
- tools：
  - `tools[].name/description/input_schema` -> `tools[].function.{name,description,parameters}`
- tool_choice：
  - `auto` -> `auto`
  - `any` -> `auto`（保留注释，后续可扩展）
  - `tool` -> `{"type":"function","function":{"name":...}}`
- output_format：
  - `type=json` + schema -> `response_format.type=json_schema`
  - schema 映射至 `json_schema.schema`，默认 `strict=true`（可配置）
- image block：
  - base64 image -> `image_url.url = "data:<media_type>;base64,<data>"`
- document block（Phase 2 仍不支持 file_id）：
  - 默认 `reject` 或 `strip`（保持 Phase 1 行为，可配置）
- thinking：
  - 若下游支持推理参数：根据预算映射到 `reasoning_effort`（low/medium/high）
  - 否则忽略并记录日志（debug）

### 5.5 响应映射细则（OpenAI -> Anthropic）
- tool_calls：
  - `choices[0].message.tool_calls[]` -> Anthropic `tool_use` block
  - `finish_reason=tool_calls` -> `stop_reason=tool_use`
- reasoning_content：
  - 若存在 `message.reasoning_content` -> Anthropic `thinking` block（放在 content 首位）
- response_format：
  - 按 OpenAI 返回 `message.content` 原样放入 `text` block
- usage：
  - `prompt_tokens` -> `input_tokens`
  - `completion_tokens` -> `output_tokens`

### 5.6 流式 SSE 转换设计
- OpenAI SSE chunk 解析：按行读取 `data:` payload
- 事件状态机：
  1. `message_start`（生成空 message 元数据）
  2. `content_block_start`（首次 content 或 reasoning/tool_use 开始）
  3. `content_block_delta`（text_delta / thinking_delta / input_json_delta）
  4. `content_block_stop`
  5. `message_delta`（finish_reason / usage）
  6. `message_stop`
- 需要维护：
  - 当前 content block 类型与 index
  - 已累计的文本（如需拼接）
  - tool_calls 的分片参数拼接（JSON 字符串）
- 终止条件：收到 `data: [DONE]` 后发送 `message_stop`

### 5.7 错误与兼容策略
- 对不支持内容类型（document/file/audio）仍返回 `invalid_request_error`
- 下游错误保持 Phase 1 映射
- 流式下游错误：立即发送 Anthropic `error` 并终止连接

### 5.8 配置项扩展
- `THINKING_MAP`：budget_tokens -> reasoning_effort 映射（可选）
- `OUTPUT_STRICT`：结构化输出 strict 默认值（true/false）
- `ALLOW_IMAGES`：是否允许 image block（默认 true）
- `DOCUMENT_POLICY`：reject/strip/text_only（延续 Phase 1 配置策略）

### 5.9 测试计划（Phase 2）
- 单元测试：
  - tools 映射
  - tool_calls -> tool_use
  - reasoning_content -> thinking
  - output_format -> response_format
- 集成测试：
  - 模拟 OpenAI SSE 流式 chunk，验证 Anthropic SSE 输出
  - 图片 base64 映射 data URL
  - tool_calls 分片拼接正确性

---

## 6. Phase 2 可执行子任务清单（按文件与函数）

### 6.1 数据结构与模型
- `src/models.rs`
  - 拆分或扩展：新增 OpenAI/Anthropic 的工具、结构化输出、流式 chunk 结构体
  - Anthropic content block：支持 `tool_use`, `tool_result`, `thinking`, `redacted_thinking`, `image`, `document`
  - OpenAI message：支持多模态 `content`（text/image_url/file）
  - OpenAI tool_calls：`OpenAIToolCall`, `OpenAIToolCallFunction`
  - OpenAI stream chunk：`OpenAIChunk`, `OpenAIChunkChoice`, `OpenAIDelta`

### 6.2 配置
- `src/config.rs`
  - 新增配置读取：`OUTPUT_STRICT`, `ALLOW_IMAGES`, `DOCUMENT_POLICY`, `THINKING_MAP`
  - 新增结构体字段：`output_strict`, `allow_images`, `document_policy`, `thinking_map`

### 6.3 请求转换（Anthropic -> OpenAI）
- `src/translate.rs`
  - 新增 `anthropic_tools_to_openai_tools(...)`
  - 新增 `anthropic_tool_choice_to_openai(...)`
  - 新增 `anthropic_output_format_to_openai(...)`
  - 扩展 `extract_content_text(...)` 为 `extract_content_parts(...)`
    - text -> text part
    - image -> image_url part（data URL）
    - tool_result -> OpenAI role=tool message
  - 处理 `thinking`：映射到 `reasoning_effort`（如配置）

### 6.4 响应转换（OpenAI -> Anthropic）
- `src/translate.rs`
  - `openai_to_anthropic(...)` 支持：
    - tool_calls -> tool_use blocks
    - reasoning_content -> thinking block
    - non-text content 的降级策略（若出现则 error）

### 6.5 SSE 流式转换
- 新增 `src/streaming.rs`
  - `openai_stream_to_anthropic_sse(...)`：解析 OpenAI SSE，输出 Anthropic SSE
  - 实现状态机：message_start / content_block_start / delta / stop / message_delta / message_stop
  - 处理 tool_calls 分片拼接
  - 处理 reasoning_content delta

### 6.6 HTTP Handler
- `src/handlers.rs`
  - 当 `stream=true` 时走 streaming 路径
  - 非流式保留现有逻辑
  - 增加流式错误处理（立即写入 error 并终止）

### 6.7 测试
- `src/translate.rs` 与 `src/streaming.rs` 添加单元测试
  - tools 映射
  - tool_calls -> tool_use
  - reasoning_content -> thinking
  - output_format -> response_format
  - SSE chunk -> Anthropic SSE 顺序与内容
  - tool_calls 分片合并

---

## 7. Phase 2 子任务拆分（含输入/输出/验收标准）

### 7.1 数据结构扩展（models）
- 输入：现有 `src/models.rs`
- 输出：新增/扩展 Anthropic/OpenAI 结构体
- 验收：
  - Anthropic content block 覆盖 text/tool_use/tool_result/thinking/redacted_thinking/image/document
  - OpenAI message 支持 `content` 多模态数组
  - OpenAI tool_calls 与 stream chunk 结构体可序列化/反序列化

### 7.2 配置扩展（config）
- 输入：环境变量
- 输出：`Config` 新字段：`output_strict`、`allow_images`、`document_policy`、`thinking_map`
- 验收：
  - 变量缺失时有默认值
  - 非法值返回清晰错误

### 7.3 tools 映射
- 输入：Anthropic tools/tool_choice
- 输出：OpenAI tools/tool_choice
- 验收：
  - name/description/input_schema 正确映射到 function.parameters
  - tool_choice=auto/any/tool 三种模式覆盖

### 7.4 output_format 映射
- 输入：Anthropic output_format
- 输出：OpenAI response_format
- 验收：
  - json schema 映射正确
  - strict 受 `OUTPUT_STRICT` 控制

### 7.5 content 映射（请求侧）
- 输入：Anthropic content blocks
- 输出：OpenAI content parts + tool messages
- 验收：
  - text block -> text part
  - image block -> image_url data URL（允许时）
  - document block 按策略 reject/strip/text_only
  - tool_result -> role=tool message

### 7.6 响应映射（工具/推理）
- 输入：OpenAI response message
- 输出：Anthropic content blocks
- 验收：
  - tool_calls -> tool_use blocks
  - reasoning_content -> thinking block（位于首位）
  - finish_reason 映射正确

### 7.7 SSE 转换（streaming）
- 输入：OpenAI SSE chunks（data: JSON）
- 输出：Anthropic SSE 事件流
- 验收：
  - 事件顺序符合 message_start → content_block_* → message_delta → message_stop
  - tool_calls 参数分片可正确拼接为完整 JSON
  - data: [DONE] 正确终止

### 7.8 Handler 流式分支
- 输入：`stream=true` 的请求
- 输出：SSE 代理响应
- 验收：
  - 非流式逻辑不受影响
  - 下游错误时返回 Anthropic error 并中止

### 7.9 测试补齐
- 输入：fixtures / mock chunks
- 输出：新增单元测试与最小集成测试
- 验收：
  - 覆盖 tools/output_format/streaming/tool_calls 分片
    - 关键边界：空 chunk、缺字段、非法 block 类型

---

## 8. Phase 3 详细计划：可观测性、性能与配置增强

### 8.1 目标
- 提供可观测性闭环（日志/指标/追踪）
- 提升稳定性与吞吐（连接池、超时、限流）
- 支持更灵活配置（动态配置、路由、多下游）

### 8.2 可观测性（Observability）
#### 8.2.1 日志规范化
- 统一字段：request_id、model、latency_ms、upstream_status、downstream_status
- 支持结构化日志（JSON）
- 允许 `LOG_LEVEL`、`LOG_FORMAT`（text/json）
- 记录下游响应时间与错误类型

#### 8.2.2 指标（Metrics）
- 暴露 `/metrics`（Prometheus 格式）
- 关键指标：
  - QPS / RPS
  - 请求延迟直方图（p50/p95/p99）
  - 下游错误率
  - 流式连接数
  - 转换失败次数

#### 8.2.3 Trace
- 引入 OpenTelemetry（可选）
- 透传上游请求 ID 或生成 trace id

### 8.3 性能优化
#### 8.3.1 连接池与超时
- Reqwest client 统一实例化
- 设置连接池大小（`POOL_MAX_IDLE_PER_HOST`）
- 超时配置（`CONNECT_TIMEOUT_MS` / `READ_TIMEOUT_MS`)

#### 8.3.2 流式优化
- SSE 代理时减少内存拷贝
- 限制缓冲区大小避免 OOM

#### 8.3.3 限流与并发
- 可选：token bucket / semaphore
- 配置 `MAX_INFLIGHT` 并发请求数

### 8.4 配置增强
#### 8.4.1 动态配置
- 支持加载 `config.json` 或 `config.yaml`
- 可选热加载（SIGHUP 或定时刷新）

#### 8.4.2 多下游路由
- 支持多下游 base_url + key
- 基于 model 前缀或映射表路由
- 简单负载均衡（round-robin）

#### 8.4.3 安全与合规
- API key 保密（日志脱敏）
- 黑名单/白名单模型

### 8.5 Phase 3 子任务拆分（按模块）
- `src/config.rs`
  - 支持文件配置与热加载
  - 新增连接池/超时/并发参数
- `src/main.rs`
  - 初始化 metrics/trace
- `src/handlers.rs`
  - 注入 request_id / trace_id
  - 统计 latency / status
- `src/metrics.rs`
  - Prometheus exporter / counters / histograms
- `src/router.rs`
  - 多下游路由逻辑

### 8.6 验收标准
- `/metrics` 可访问且包含核心指标
- 可通过配置调整超时和并发
- 支持至少 2 个下游路由

---

## 9. Phase 3 子任务拆分（输入/输出/验收标准）

### 9.1 日志规范化
- 输入：`LOG_LEVEL` / `LOG_FORMAT` / 请求上下文字段
- 输出：结构化日志（JSON 或 text）
- 验收：
  - 每条请求日志包含 request_id、model、latency_ms、status
  - 下游错误带 error_type 与 upstream/downstream 状态

### 9.2 指标与监控
- 输入：请求/响应生命周期事件
- 输出：`/metrics`（Prometheus）
- 验收：
  - 有请求计数、错误计数、延迟直方图
  - 流式连接数可观察

### 9.3 Trace 支持（可选）
- 输入：上游 header 或内部生成 trace id
- 输出：OpenTelemetry span
- 验收：
  - trace id 可在日志中关联

### 9.4 连接池与超时
- 输入：`CONNECT_TIMEOUT_MS` / `READ_TIMEOUT_MS` / `POOL_MAX_IDLE_PER_HOST`
- 输出：reqwest client 配置
- 验收：
  - 超时可配置且生效
  - 连接池复用生效

### 9.5 并发与限流
- 输入：`MAX_INFLIGHT`
- 输出：并发控制（Semaphore 或 Token Bucket）
- 验收：
  - 达到上限后返回 429

### 9.6 动态配置
- 输入：`CONFIG_PATH`
- 输出：配置文件加载（JSON/YAML）
- 验收：
  - 文件改动可热加载（或 SIGHUP）
  - 无效配置不覆盖当前配置

### 9.7 多下游路由
- 输入：下游配置列表（base_url + key + 权重）
- 输出：按模型路由或轮询
- 验收：
  - 至少 2 个下游可用
  - 路由规则可配置

### 9.8 安全与合规
- 输入：`MODEL_ALLOWLIST` / `MODEL_BLOCKLIST`
- 输出：请求过滤
- 验收：
  - 不允许模型返回 400 + invalid_request_error

---

## 10. 新特性：Anthropic `/v1/models` 代理

### 10.1 目标
- 对外提供 `GET /v1/models`（Anthropic 规范）
- 下游对接 OpenAI `/v1/models`
- 转换字段并补齐 `display_name`

### 10.2 映射规则
| OpenAI 字段 | Anthropic 字段 | 规则 |
| --- | --- | --- |
| `id` | `id` | 透传 |
| `object` | `type` | 固定转为 `model` |
| `created` | `created_at` | Unix 秒 -> ISO 8601 UTC |
| `owned_by` | - | 丢弃 |
| - | `display_name` | 映射表或规则生成 |

### 10.3 display_name 策略
优先级：\n
1. `MODEL_DISPLAY_MAP`（JSON map）\n
2. 基于 model id 的分词/标题化规则\n
3. 回退为原始 `id`\n

### 10.4 新增配置
- `MODEL_DISPLAY_MAP`（JSON map，示例：`{\"gpt-4o-mini\":\"GPT-4o Mini\"}`）
- `MODELS_JSON`（JSON 数组，直接指定 `/v1/models` 返回列表，优先于下游）

### 10.5 代码改动点
- `src/models.rs`：新增 OpenAI/Anthropic models 结构体
- `src/handlers.rs`：新增 `get_models` handler
- `src/translate.rs`：新增 `openai_models_to_anthropic` 转换
- `src/main.rs`：路由新增 `GET /v1/models`
- `src/config.rs`：读取 `MODELS_JSON` 并缓存为 Anthropic models 列表

### 10.6 测试
- 单元测试：模型映射 / 时间戳转换 / display_name
- 集成测试：mock OpenAI `/v1/models` 返回
