# Anthropic 直转发默认方案计划

目标：在现有网关中新增“Anthropic 格式请求直转发”能力，作为默认行为；同时保留“Anthropic → OpenAI 兼容格式转换转发”的可选路径。并在配置缺失关键参数时严格报错，不自动回退。

## 一、现状简述
- `/v1/messages` 当前仅支持 Anthropic 请求解析后转 OpenAI 兼容格式，再转回 Anthropic 响应。
- `/v1/models` 已支持 OpenAI → Anthropic 的响应映射。
- 流式响应通过 `streaming.rs` 将 OpenAI SSE 转成 Anthropic SSE。

## 二、总体设计

### 2.1 运行模式
在配置中引入转发模式：
- `passthrough`：Anthropic 原格式直转发（默认）。
- `translate`：Anthropic → OpenAI 兼容格式转换转发（现有）。

### 2.2 直转发关键要求
- 不对请求体做严格结构反序列化，避免拒绝 Anthropic 新增字段（如 `top_k`、`metadata`）。
- 必须保留 Anthropic 头部：`x-api-key`、`anthropic-version`、可选 `anthropic-beta`。
- SSE 流式响应原样透传，不进行 chunk 解析和重组。

### 2.3 严格配置校验
- 当 `forward_mode=passthrough` 时，`downstream.anthropic_version` 不能为空，否则启动失败。
- 不自动回退到 `translate`。

## 三、配置设计

### 3.1 新增配置结构
建议在现有 `Config` 中增加：
- `anthropic.forward_mode`: `passthrough` | `translate`，默认 `passthrough`。
- `downstream.anthropic_version`: string，默认 `2023-06-01`。
- `downstream.anthropic_beta`: 可选 string 或 string[]。

### 3.2 `config.yaml` 示例（默认直转发）
```yaml
anthropic:
  forward_mode: "passthrough"

downstream:
  base_url: "https://api.anthropic.com"
  api_key: "sk-ant-api03-xxxx"
  anthropic_version: "2023-06-01"
  anthropic_beta: "structured-outputs-2025-11-13"
```

### 3.3 严格校验逻辑
在 `Config::normalize()` 中添加：
- 如果 `anthropic.forward_mode == passthrough` 且 `downstream.anthropic_version` 为空，返回错误并终止启动。

## 四、接口行为设计

### 4.1 `/v1/messages` 非流式
分支逻辑：
1. `passthrough`：
   - 读取 JSON 原始请求体（`serde_json::Value`）。
   - 提取并校验 `model` 字段，执行 allowlist/blocklist 与 model_map 映射。
   - 组装 Anthropic 请求头并直发 `POST {base_url}/v1/messages`。
   - 直接透传 JSON 响应体给上游客户端。
2. `translate`：
   - 现有逻辑不变（`anthropic_to_openai` → OpenAI 下游 → `openai_to_anthropic`）。

### 4.2 `/v1/messages` 流式（SSE）
分支逻辑：
1. `passthrough`：
   - 直连 `POST {base_url}/v1/messages`，`stream=true`。
   - `bytes_stream()` 原样透传到客户端，不做 `data:` 解析，不做 `content_block` 重建。
2. `translate`：
   - 继续使用 `streaming.rs`（OpenAI SSE → Anthropic SSE）。

### 4.3 `/v1/models`
- `passthrough`：直连 `GET {base_url}/v1/models`，使用 Anthropic 头，透传响应。
- `translate`：维持现有 OpenAI → Anthropic 映射逻辑。

## 五、数据模型调整

### 5.1 `AnthropicRequest` 字段补齐（可选但推荐）
- 增加 `top_k: Option<u32>` 等规范中的字段，以提升 `translate` 模式兼容性。
- 直转发模式不依赖该结构，因此不强制。

## 六、错误处理策略

### 6.1 直转发错误
- Anthropic 上游返回非 2xx 时，保留原始 status + body 透传。
- 请求发送失败或响应体不可读时，返回 `AppError::api_error`。

### 6.2 现有错误映射
- `translate` 模式仍使用 `map_downstream_error`。

## 七、观测与日志

- `dump_downstream=true` 时：
  - `passthrough` 记录原始请求 JSON 与原始响应体，不做解析拼接。
  - SSE 流式只记录原始 chunk（可按需截断）。
- `translate` 模式维持现有追踪字段。

## 八、实现步骤（建议顺序）

1. **配置与校验**
   - 扩展 `Config` 与 `config.yaml` 示例。
   - `normalize()` 增加严格校验逻辑。

2. **路由处理分支**
   - 在 `handlers::post_messages` 引入 `forward_mode` 分支。
   - 使用 `serde_json::Value` 作为入口，最小字段抽取（`model` / `stream`）。

3. **直转发非流式请求**
   - `reqwest::Client` 直连 Anthropic。
   - 设置 Anthropic 头部并透传响应。

4. **直转发流式 SSE**
   - 新增 `stream_anthropic_passthrough`（独立函数或模块）。
   - `bytes_stream()` 原样输出。

5. **/v1/models 直转发**
   - `get_models` 增加 `forward_mode` 分支。

6. **模型映射与访问控制**
   - 直转发路径保留 allowlist/blocklist 与 model_map 逻辑。
   - JSON 体内 `model` 字段做更新后再发送。

7. **测试补充**
   - 直转发模式下：
     - 允许未知字段（如 `top_k`）。
     - `model_map` 映射生效。
     - SSE chunk 透传。
   - 错误透传：上游 401/403/429 等状态码不被改写。

## 九、涉及文件
- `src/config.rs`
- `config.yaml`
- `src/handlers.rs`
- `src/streaming.rs` 或新建 `src/streaming_passthrough.rs`
- `src/models.rs`
- `src/error.rs`
- `src/main.rs`（若新增配置结构或路由逻辑）

