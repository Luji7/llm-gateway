# Anthropic Messages API 消息格式规范

**最后更新**: 2025年初  
**适用模型**: Claude 3.5/3.7, Claude 4 全系列 (Opus 4.5, Sonnet 4.5, Haiku 4.5)

---

## 1. 基础信息

### 1.1 接入点

POST https://api.anthropic.com/v1/messages


### 1.2 请求头 (Headers)
| Header | 说明 |
|--------|------|
| `anthropic-version` | **必需**。API 版本，当前为 `2023-06-01` |
| `x-api-key` | **必需**。API Key (`sk-ant-api03-...`) |
| `content-type` | **必需**。固定为 `application/json` |
| `anthropic-beta` | 可选。功能开关，如 `prompt-caching-2024-07-31`, `structured-outputs-2025-11-13` |

---

## 2. 请求格式 (Request)

### 2.1 基础请求体
```json
{
  "model": "claude-sonnet-4-5",
  "max_tokens": 4096,
  "messages": [
    {
      "role": "user",
      "content": "Hello!"
    }
  ],
  "system": "You are a helpful assistant.",
  "temperature": 1.0,
  "stream": false
}
```

### 2.2 核心字段详解

| 字段               | 类型           | 必填 | 说明                                                    |
| ---------------- | ------------ | -- | ----------------------------------------------------- |
| `model`          | string       | ✅  | 模型 ID，如 `claude-opus-4-20250514`, `claude-sonnet-4-5` |
| `messages`       | array        | ✅  | 对话历史数组                                                |
| `max_tokens`     | integer      | ✅  | 最大生成 token 数 (1-128000，取决于模型)                         |
| `system`         | string/array | ❌  | 系统提示，支持字符串或多文本块数组                                     |
| `temperature`    | number       | ❌  | 采样温度 (0-1)，**启用 thinking 时必须设为 1**                    |
| `top_p`          | number       | ❌  | 核采样阈值 (0-1)                                           |
| `top_k`          | integer      | ❌  | Top-k 采样                                              |
| `stop_sequences` | array        | ❌  | 自定义停止序列字符串数组                                          |
| `stream`         | boolean      | ❌  | 是否启用 SSE 流式输出                                         |
| `tools`          | array        | ❌  | 工具定义数组                                                |
| `tool_choice`    | object       | ❌  | 工具选择策略 (`auto`/`any`/`tool`)                          |
| `thinking`       | object       | ❌  | 扩展思考模式配置                                              |
| `output_format`  | object       | ❌  | 结构化输出格式 (需 beta header)                               |

## 3. 消息内容格式

### 3.1 消息角色 (Role)

只允许: "user" | "assistant"
⚠️ 注意: Anthropic API 没有 role: "system"，系统提示必须通过顶层 system 字段传递

### 3.2 Content 结构

Content 可以是字符串，或 Content Block 数组：

#### 简单格式（字符串）

```json
{
  "role": "user",
  "content": "Hello, Claude!"
}
```

#### 复杂格式（Content Block 数组）

支持多模态内容，每种 block 必须有 type 字段：

##### 文本块 (Text)

```json
{
  "type": "text",
  "text": "What's in this image?",
  "cache_control": { "type": "ephemeral", "ttl": "1h" }  // 可选：缓存控制
}
```

##### 图片块 (Image)

```json
{
  "type": "image",
  "source": {
    "type": "base64",
    "media_type": "image/jpeg",
    "data": "/9j/4AAQSkZJRg..."  // base64 编码
  }
}
```

###### 文档块 (Document)

```json
{
  "type": "document",
  "source": {
    "type": "base64",
    "media_type": "application/pdf",
    "data": "...",
    "cache_control": { "type": "ephemeral" }
  }
}
```

##### 工具结果块 (Tool Result)

```json
{
  "type": "tool_result",
  "tool_use_id": "toolu_01A...",
  "content": "72°F and sunny",
  "is_error": false
}
```

## 4. 高级功能配置

### 4.1 Extended Thinking（扩展思考模式）

仅 Claude 3.7+ 及 Claude 4 系列支持：

```json
{
  "model": "claude-opus-4",
  "max_tokens": 20000,
  "thinking": {
    "type": "enabled",
    "budget_tokens": 16000
  },
  "temperature": 1,  // ⚠️ 启用 thinking 时必须设为 1
  "messages": [...]
}
```

### 4.2 System 提示数组格式（支持缓存控制）

```json
{
  "system": [
    {
      "type": "text",
      "text": "You are a helpful coding assistant.",
      "cache_control": { "type": "ephemeral" }
    },
    {
      "type": "text",
      "text": "Today is 2025-01-31."
    }
  ]
}
```

### 4.3 工具定义 (Tools)

```json
{
  "tools": [
    {
      "name": "get_weather",
      "description": "Get weather information",
      "input_schema": {
        "type": "object",
        "properties": {
          "location": { "type": "string" },
          "unit": { "enum": ["celsius", "fahrenheit"] }
        },
        "required": ["location"]
      }
    }
  ],
  "tool_choice": { "type": "auto" }  // auto | any | tool
}
```

### 4.4 结构化输出 (Structured Outputs)

需添加 Header: anthropic-beta: structured-outputs-2025-11-13

```json
{
  "model": "claude-sonnet-4-5",
  "output_format": {
    "type": "json",
    "schema": {
      "type": "object",
      "properties": {
        "name": { "type": "string" },
        "ingredients": { "type": "array", "items": { "type": "string" } }
      },
      "required": ["name", "ingredients"]
    }
  }
}
```

## 5. 响应格式 (Response)

### 5.1 基础响应结构（非流式）

```json
{
  "id": "msg_01AbCdEfGhIjKlMnOpQrStUv",
  "type": "message",
  "role": "assistant",
  "model": "claude-sonnet-4-5-20250514",
  "content": [
    {
      "type": "text",
      "text": "Hello! How can I help you today?"
    }
  ],
  "stop_reason": "end_turn",
  "stop_sequence": null,
  "usage": {
    "input_tokens": 15,
    "output_tokens": 25,
    "cache_creation_input_tokens": 0,
    "cache_read_input_tokens": 0
  }
}
```

字段说明：

| 字段              | 说明                                                          |
| --------------- | ----------------------------------------------------------- |
| `id`            | 消息 ID (以 `msg_` 开头)                                         |
| `type`          | 固定为 `"message"`                                             |
| `content`       | **Content Block 数组**                                        |
| `stop_reason`   | `end_turn` \| `max_tokens` \| `stop_sequence` \| `tool_use` |
| `stop_sequence` | 触发的停止序列（如适用）                                                |
| `usage`         | Token 使用统计，包含缓存相关字段                                         |

### 5.2 Content Block 类型详解

#### 5.2.1 Text Block（标准文本）

```json
{
  "type": "text",
  "text": "This is the response content..."
}
```

#### 5.2.2 Tool Use Block（工具调用）

当 stop_reason 为 "tool_use" 时出现：

```json
{
  "type": "tool_use",
  "id": "toolu_01AbcD123eFgH456iJkL789mN",
  "name": "get_weather",
  "input": {
    "location": "London, UK",
    "unit": "celsius"
  }
}
```

⚠️ 重要: 后续请求必须将该 block 原样放入消息历史，并将工具结果通过 tool_result 类型 block 回传。

#### 5.2.3 Thinking Block（扩展思考）

启用 thinking 模式时出现，必须位于 content 数组首位：

```json
{
  "type": "thinking",
  "thinking": "Let me analyze this step by step...",
  "signature": "EqQCCkYICxgCKkD..."  // 加密签名
}
```

#### 5.2.4 Redacted Thinking Block（安全审查）

当思考内容触发安全加密时出现：

```json
{
  "type": "redacted_thinking",
  "data": "EmwKFY7Ld..."  // 加密数据
}
```

### 5.3 完整响应场景示例

场景 A：普通文本响应

```json
{
  "id": "msg_01XYZ",
  "type": "message",
  "role": "assistant",
  "model": "claude-sonnet-4-5",
  "content": [{"type": "text", "text": "The answer is 42."}],
  "stop_reason": "end_turn",
  "usage": { "input_tokens": 10, "output_tokens": 8, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0 }
}
```

场景 B：Thinking + 工具调用

```json
{
  "id": "msg_02ABC",
  "type": "message",
  "role": "assistant",
  "model": "claude-opus-4",
  "content": [
    {
      "type": "thinking",
      "thinking": "The user wants to calculate...",
      "signature": "abc123..."
    },
    {
      "type": "tool_use",
      "id": "toolu_calc_789",
      "name": "calculator",
      "input": { "expression": "355/113" }
    }
  ],
  "stop_reason": "tool_use",
  "usage": { "input_tokens": 50, "output_tokens": 120, "cache_creation_input_tokens": 1024, "cache_read_input_tokens": 0 }
}
```

场景 C：Thinking + 最终文本

```json
{
  "id": "msg_03DEF",
  "content": [
    {
      "type": "thinking",
      "thinking": "Analyzing the poem...",
      "signature": "sig..."
    },
    {
      "type": "text",
      "text": "This poem uses iambic pentameter..."
    }
  ],
  "stop_reason": "end_turn"
}
```

## 6. 流式响应格式 (SSE)

当设置 "stream": true 时，返回 Server-Sent Events：

```shell
event: message_start
data: {"type":"message_start","message":{"id":"msg_...","type":"message","role":"assistant","content":[],"usage":{"input_tokens":15,"output_tokens":0}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me think"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":85}}

event: message_stop
data: {"type":"message_stop"}
```

### 6.1 事件类型说明

| 事件                    | 说明                                                                          |
| --------------------- | --------------------------------------------------------------------------- |
| `message_start`       | 消息开始，包含初始 metadata                                                          |
| `content_block_start` | 新的 content block 开始                                                         |
| `content_block_delta` | content block 内容增量更新 (`thinking_delta`, `text_delta`, `input_json_delta` 等) |
| `content_block_stop`  | content block 结束                                                            |
| `message_delta`       | 消息级更新（如 `stop_reason` 变化）                                                   |
| `message_stop`        | 消息结束                                                                        |

## 7. 错误响应格式

```json
{
  "type": "error",
  "error": {
    "type": "invalid_request_error",
    "message": "messages: Unexpected role \"system\". ..."
  }
}
```

### 7.1 错误类型对照表

| HTTP 状态码 | error.type              | 说明         |
| -------- | ----------------------- | ---------- |
| 400      | `invalid_request_error` | 请求参数错误     |
| 401      | `authentication_error`  | API Key 无效 |
| 403      | `permission_error`      | 权限不足       |
| 404      | `not_found_error`       | 模型不存在      |
| 429      | `rate_limit_error`      | 速率限制       |
| 500      | `api_error`             | 服务端错误      |
| 529      | `overloaded_error`      | 服务器过载      |

### 7.2 特殊验证错误

Thinking 上下文错误（未正确回传 thinking 块）：

```shell
messages.1.content.0.type: Expected `thinking` or `redacted_thinking`, 
but found `text`. When `thinking` is enabled, a final `assistant` message 
must start with a thinking block...
```

## 8. 关键限制与最佳实践

### 8.1 使用限制

1. Thinking 模式约束：
    - 必须设置 temperature: 1
    - Content 数组必须以 thinking 或 redacted_thinking 开头
    - Tool use 调用时，thinking block 必须在 tool_use 之前
2. Prompt Caching 限制：
    - 每请求最多 4 个 cache_control 断点
    - Claude 3.5 文化学用 1024 tokens，Claude Opus 4.5/Haiku 4.5 需 4096 tokens 才触发缓存
3. 输出上限：
    - 标准模式：8,192 tokens
    - 启用 output-128k-2025-02-19 beta：最高 128,000 tokens

### 8.2 执行顺序规则

1. 启用 Thinking + Tool Use 时的 Content Block 顺序：
    [thinking] → [tool_use] → [text]

2. 多轮对话中必须保留的上下文：
    - 必须完整保留上一轮响应中的 thinking 或 redacted_thinking block
    - 必须保留 tool_use block 直到返回对应的 tool_result

#### 8.3 数据回传规范

当需要继续对话（特别是工具调用后），必须将 assistant 的完整响应内容原样放入下一轮的 messages 中：

```json
{
  "role": "assistant",
  "content": [
    {
      "type": "tool_use",
      "id": "toolu_123",
      "name": "get_weather",
      "input": { "location": "London" }
    }
  ]
}
```

然后添加 tool_result：

```json
{
  "role": "user",
  "content": [
    {
      "type": "tool_result",
      "tool_use_id": "toolu_123",
      "content": "Weather: 72°F, Sunny",
      "is_error": false
    }
  ]
}
```


**文档特点：**
- 包含请求和响应的完整格式规范
- 详细说明 Claude 4 系列特有的 Thinking 模式及其约束
- 覆盖多模态内容（文本、图片、PDF）的处理方式
- 包含流式响应的 SSE 事件结构
- 提供常见的错误类型和处理建议
- 强调关键限制（如 temperature 必须为 1 才能使用 thinking）