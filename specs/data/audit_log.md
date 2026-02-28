# Audit Log JSONL 数据 Schema

本文档描述 `audit_log` 离线记录的 JSONL 格式。每行是一条完整记录，包含 **上游请求** 与 **上游响应**。

## 1. 文件格式

- **格式**: JSON Lines（JSONL）
- **编码**: UTF-8
- **记录单位**: 每行一个 JSON 对象（完整请求+响应）
- **滚动**: 当文件大小超过 `audit_log.max_file_bytes` 时，新文件以时间戳后缀创建。
  - `./logs/upstream_audit.jsonl` -> `./logs/upstream_audit.<ts>.jsonl`
  - `<ts>` 为 Unix 毫秒时间戳

## 2. 顶层字段

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `ts_start_ms` | number | ✅ | 请求开始时间（Unix 毫秒） |
| `ts_end_ms` | number | ✅ | 响应结束时间（Unix 毫秒） |
| `request_id` | string | ✅ | 网关生成的请求 ID |
| `route` | string | ✅ | 上游路由（如 `/v1/messages`, `/v1/models`） |
| `mode` | string | ✅ | 转发模式（`passthrough` / `translate`） |
| `method` | string | ✅ | HTTP 方法（`GET`, `POST`） |
| `request` | object | ✅ | 上游请求对象（见 3.1） |
| `response` | object | ✅ | 上游响应对象（见 3.2） |
| `meta` | object | ✅ | 辅助元信息（见 3.3） |

## 3. 子对象结构

### 3.1 `request` 对象

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `headers` | object | ✅ | 请求头键值对（全部字符串；`Authorization` 已脱敏） |
| `body` | any (JSON) | ✅ | 请求体 JSON 对象/数组/值。记录的是**上游原始请求体**（`/v1/messages` 为 Anthropic 格式；`/v1/models` 为 `null`）。不会记录为字符串。 |

### 3.2 `response` 对象

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `status` | number | ✅ | HTTP 状态码 |
| `headers` | object | ✅ | 响应头键值对（全部字符串；`Authorization` 已脱敏） |
| `body` | any (JSON) | ✅ | 响应体 JSON 对象/数组/值。非流式为实际响应 JSON；流式会聚合输出并尝试反序列化。解析失败则为 `null`。 |

### 3.3 `meta` 对象

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `model` | string \| null | ❌ | 业务模型名（若可用） |
| `stream` | boolean \| null | ❌ | 是否流式请求 |
| `body_truncated` | boolean | ✅ | 是否触发 `max_body_bytes` 截断 |
| `body_parse_error` | boolean | ✅ | 响应体 JSON 解析失败标记（仅针对 **response.body**） |

## 4. 脱敏规则

- `Authorization` 头始终记录为 `[redacted]`。
- 其他头不做脱敏（如需可扩展）。

## 5. body 数据来源与格式细则

### 5.1 上游请求 `request.body`
- `/v1/messages`：记录为 **上游原始请求体**（JSON Value），结构遵循 **Anthropic Messages API**。核心字段包括但不限于：  
  - `model` (string, 必填)  
  - `messages` (array, 必填)  
  - `max_tokens` (integer, 必填)  
  - `system` (string | array, 可选)  
  - `temperature` / `top_p` / `top_k` (number/integer, 可选)  
  - `stream` (boolean, 可选)  
  - `tools` / `tool_choice` / `thinking` / `output_format` (object/array, 可选)  
  `messages[*].content` 可为 string 或 content blocks 数组（如 text/image/document/tool_result 等）。  
  详见 `specs/anthropic消息格式.md`。
  - `passthrough` 与 `translate` 模式一致，均为 Anthropic Messages 格式（未做转换）。
- `/v1/models`：固定为 `null`（无请求体）。

### 5.2 上游响应 `response.body`
- **非流式**：
  - 读取完整响应体并反序列化为 JSON Value。
  - 解析失败时：`body = null`，`meta.body_parse_error = true`。
- **流式**：
  - 聚合流式输出为完整响应内容后再反序列化。
  - 解析失败时：`body = null`，`meta.body_parse_error = true`。

### 5.3 模式差异（`response.body` 结构）
- `passthrough`：`response.body` 为 **下游原始响应**（Anthropic Messages Response）解析后的 JSON。  
  常见字段：`id`, `type`, `role`, `model`, `content`(blocks), `stop_reason`, `usage` 等。  
  详见 `specs/anthropic消息格式.md`。
- `translate`：`response.body` 为 **网关转换后的上游响应**（Anthropic 格式）解析后的 JSON，结构同上。

### 5.4 `/v1/models` 响应体
- `passthrough`：`response.body` 为下游 Anthropic `/v1/models` 响应 JSON（`data` 数组，含 `id/type/display_name/created_at`）。  
  详见 `specs/anthropic models端点消息格式.md`。
- `translate`：`response.body` 为网关转换后的 Anthropic 模型列表 JSON（结构与上相同）。

## 6. 解析与失败策略

- **只对响应体进行 JSON 解析错误标记**（`body_parse_error`）。  
- 截断发生时：  
  - `meta.body_truncated = true`  
  - 若解析失败，仍遵循 `body = null`  

## 7. 响应头记录差异

- **非流式**：记录真实响应头（由下游返回）。  
- **流式**：仅记录 `content-type`（若存在），其余响应头不一定保留。  

## 8. 示例记录

```json
{
  "ts_start_ms": 1772207155925,
  "ts_end_ms": 1772207156279,
  "request_id": "req-1772207155925-9",
  "route": "/v1/messages",
  "mode": "passthrough",
  "method": "POST",
  "request": {
    "headers": {
      "content-type": "application/json",
      "x-api-key": "sk-***",
      "authorization": "[redacted]"
    },
    "body": {
      "model": "claude-sonnet-4-5",
      "max_tokens": 128,
      "messages": [
        {"role": "user", "content": "Hello"}
      ]
    }
  },
  "response": {
    "status": 200,
    "headers": {
      "content-type": "application/json"
    },
    "body": {
      "id": "msg_01",
      "type": "message",
      "role": "assistant",
      "content": [
        {"type": "text", "text": "Hi"}
      ]
    }
  },
  "meta": {
    "model": "claude-sonnet-4-5",
    "stream": true,
    "body_truncated": false,
    "body_parse_error": false
  }
}
```
