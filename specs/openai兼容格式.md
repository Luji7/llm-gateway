# OpenAI Chat Completions API 消息格式规范

**版本**: 2025-08 (GA)  
**协议**: HTTP/1.1 或 HTTP/2  
**字符编码**: UTF-8

---

## 1. 接口端点

POST https://api.openai.com/v1/chat/completions
Authorization: Bearer {API_KEY}
Content-Type: application/json


---

## 2. 请求格式（Request）

### 2.1 完整请求体示例

```json
{
  "model": "gpt-4o-mini",
  "messages": [
    {
      "role": "system",
      "content": "You are a helpful assistant."
    },
    {
      "role": "user",
      "name": "Samuel",
      "content": "Hello, how are you?"
    }
  ],
  "max_completion_tokens": 4096,
  "temperature": 1,
  "top_p": 1,
  "frequency_penalty": 0,
  "presence_penalty": 0,
  "tools": [...],
  "tool_choice": "auto",
  "parallel_tool_calls": false,
  "response_format": {"type": "text"},
  "stream": false,
  "store": false
}
```

### 2.2 核心参数说明

| 字段                      | 类型      | 必填 | 说明                                                     |
| ----------------------- | ------- | -- | ------------------------------------------------------ |
| `model`                 | string  | ✅  | 模型 ID，如 `gpt-4o`、`gpt-4.1-mini`、`o3-mini`、`gpt-5`      |
| `messages`              | array   | ✅  | 对话历史消息数组                                               |
| `max_completion_tokens` | integer | ❌  | **输出 token 上限**（推理模型如 o1/o3 必须使用此字段，废弃旧版 `max_tokens`） |
| `stream`                | boolean | ❌  | 是否启用 SSE 流式传输，默认 `false`                               |

### 2.3 消息（Message）结构

消息对象支持以下角色 role：

| 角色          | 适用模型                   | 说明                          |
| ----------- | ---------------------- | --------------------------- |
| `system`    | GPT-4/GPT-4o 系列        | 系统指令，设定助手行为                 |
| `developer` | o1/o3/o4-mini/gpt-5 系列 | **推理模型专用**，替代 `system` 角色   |
| `user`      | 全部                     | 用户输入，支持文本/图文/文件             |
| `assistant` | 全部                     | 助手回复，可包含 `tool_calls`       |
| `tool`      | 全部                     | 工具函数返回结果，需对应 `tool_call_id` |

#### Content 格式

##### 纯文本模式：

```json
{
  "role": "user",
  "content": "你好，请介绍一下自己"
}
```

##### 多模态数组模式（支持图片、PDF、音频）：

```json
{
  "role": "user",
  "content": [
    {
      "type": "text",
      "text": "描述这张图片的内容"
    },
    {
      "type": "image_url",
      "image_url": {
        "url": "https://example.com/image.png",
        "detail": "high"
      }
    },
    {
      "type": "file",
      "file": {
        "file_id": "file-abc123",
        "format": "utf-8"
      }
    }
  ]
}
```

### 2.4 采样控制参数（非推理模型）

⚠️ 以下参数仅适用于 GPT-4/GPT-4o 系列，o1/o3 等推理模型不支持：

| 参数                  | 类型           | 范围       | 默认值  | 说明                                 |
| ------------------- | ------------ | -------- | ---- | ---------------------------------- |
| `temperature`       | float        | 0-2      | 1    | 采样温度，0 为确定性输出                      |
| `top_p`             | float        | 0-1      | 1    | 核采样（Nucleus Sampling）              |
| `frequency_penalty` | float        | -2.0~2.0 | 0    | 频率惩罚，降低重复 token 概率                 |
| `presence_penalty`  | float        | -2.0~2.0 | 0    | 存在惩罚，已出现 token 降权                  |
| `logit_bias`        | object       | -        | null | 调整特定 token 采样权重 `{token_id: bias}` |
| `stop`              | string/array | -        | null | 停止序列，遇到则终止生成                       |

### 2.5 推理控制参数（推理模型专用）

适用于 o1、o3-mini、o4-mini、gpt-5 等推理模型：

| 参数                 | 类型     | 可选值                       | 说明                 |
| ------------------ | ------ | ------------------------- | ------------------ |
| `reasoning_effort` | string | `low` / `medium` / `high` | 推理努力程度（o1/o3 系列）   |
| `verbosity`        | string | `low` / `medium` / `high` | 输出详细程度（gpt-5 系列支持） |

### 2.6 工具调用（Function Calling）

```json
{
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "get_weather",
        "description": "获取指定城市的天气信息",
        "parameters": {
          "type": "object",
          "properties": {
            "location": {
              "type": "string",
              "description": "城市名称，如 Beijing"
            },
            "unit": {
              "type": "string",
              "enum": ["c", "f"],
              "description": "温度单位"
            }
          },
          "required": ["location"]
        }
      }
    }
  ],
  "tool_choice": "auto",
  "parallel_tool_calls": false
}
```

tool_choice 选项：
- "auto"：模型决定是否调用工具（默认）
- "none"：禁用工具调用
- {"type": "function", "function": {"name": "get_weather"}}：强制指定工具

### 2.7 结构化输出（Structured Outputs）

```json
{
  "response_format": {
    "type": "json_schema",
    "json_schema": {
      "name": "math_response",
      "schema": {
        "type": "object",
        "properties": {
          "steps": {
            "type": "array",
            "items": {
              "type": "object",
              "properties": {
                "explanation": {"type": "string"},
                "output": {"type": "string"}
              },
              "required": ["explanation", "output"]
            }
          },
          "final_answer": {"type": "string"}
        },
        "required": ["steps", "final_answer"]
      },
      "strict": true
    }
  }
}
```

类型选项：
- "text"：默认文本输出
- "json_object"：旧版 JSON 模式（不保证格式有效性）
- "json_schema"：严格模式，生成符合 JSON Schema 的有效数据（推荐）

### 2.8 其他高级参数

| 参数                 | 类型      | 说明                                               |
| ------------------ | ------- | ------------------------------------------------ |
| `stream_options`   | object  | 流式选项，如 `{"include_usage": true}` 在流式最后返回 usage   |
| `store`            | boolean | `false` 时禁止服务器端存储请求（默认行为可能变化）                    |
| `service_tier`     | string  | `"default"` / `"flex"` / `"priority"`，部分模型支持加速处理 |
| `prompt_cache_key` | string  | 缓存键，用于上下文 KV 缓存复用                                |
| `modalities`       | array   | `["text", "audio"]`，音频模型专用                       |
| `audio`            | object  | 音频输出配置，如 `{"voice": "alloy", "format": "wav"}`   |

## 3. 响应格式（Response）

### 3.1 非流式响应（Standard Response）

```json
{
  "id": "chatcmpl-B9L3HqD2xP6i1XzK",
  "object": "chat.completion",
  "created": 1729123456,
  "model": "gpt-4o-mini-2025-08-01",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "I'm doing well, thank you for asking! How can I assist you today?",
        "refusal": null,
        "tool_calls": null
      },
      "finish_reason": "stop",
      "logprobs": null
    }
  ],
  "usage": {
    "prompt_tokens": 25,
    "completion_tokens": 12,
    "total_tokens": 37,
    "prompt_tokens_details": {
      "cached_tokens": 10,
      "audio_tokens": 0
    },
    "completion_tokens_details": {
      "reasoning_tokens": 0,
      "audio_tokens": 0,
      "accepted_prediction_tokens": 0,
      "rejected_prediction_tokens": 0
    }
  },
  "service_tier": "default",
  "system_fingerprint": "fp_5d2a"
}
```

### 3.2 流式响应（Streaming / SSE）

流式响应以 Server-Sent Events (SSE) 形式返回，每个数据块：

```json
data: {
  "id": "chatcmpl-xxx",
  "object": "chat.completion.chunk",
  "created": 1729123456,
  "model": "gpt-4o-mini",
  "choices": [
    {
      "index": 0,
      "delta": {
        "role": "assistant",
        "content": " doing"
      },
      "finish_reason": null,
      "logprobs": null
    }
  ],
  "usage": null
}
```

**终止标记**：
data: [DONE]

### 3.3 响应字段详解

#### 顶层元数据

| 字段                   | 类型      | 说明                                               |
| -------------------- | ------- | ------------------------------------------------ |
| `id`                 | string  | 唯一响应 ID（格式：`chatcmpl-<random>`）                  |
| `object`             | string  | 对象类型：`chat.completion` / `chat.completion.chunk` |
| `created`            | integer | Unix 时间戳（秒）                                      |
| `model`              | string  | 实际使用的模型版本                                        |
| `system_fingerprint` | string  | 模型配置指纹，追踪后端配置变更                                  |

#### Choices 数组

| 字段                  | 类型      | 说明                         |
| ------------------- | ------- | -------------------------- |
| `index`             | integer | 选择项索引（通常为 0）               |
| `message` / `delta` | object  | 非流式为 `message`，流式为 `delta` |
| `finish_reason`     | string  | 生成结束原因（见下表）                |
| `logprobs`          | object  | 对数概率信息（如请求时指定）             |

#### Message / Delta 对象结构

| 字段                  | 类型          | 说明               |
| ------------------- | ----------- | ---------------- |
| `role`              | string      | 固定为 `assistant`  |
| `content`           | string/null | 生成的文本内容          |
| `refusal`           | string/null | 拒绝内容（安全过滤或政策拒绝）  |
| `tool_calls`        | array/null  | 工具调用请求           |
| `reasoning_content` | object      | 推理模型的思考链内容（特殊字段） |

### 3.4 工具调用响应格式

当模型决定调用工具时：

```json
{
  "role": "assistant",
  "content": null,
  "tool_calls": [
    {
      "id": "call_abc123xyz",
      "type": "function",
      "function": {
        "name": "get_weather",
        "arguments": "{\"location\": \"Beijing\", \"unit\": \"c\"}"
      }
    }
  ]
}
```

字段说明：
- id：工具调用唯一标识，需原样返回给后续 tool 角色消息
- type：固定为 function
- function.name：要调用的函数名
- function.arguments：JSON 格式的参数字符串（需客户端解析）

### 3.5 推理模型特殊响应（Reasoning Content）

对于 o1、o3-mini、o4-mini 等推理模型，响应包含思考过程：

```json
{
  "choices": [
    {
      "message": {
        "role": "assistant",
        "content": "The answer is 42.",
        "reasoning_content": {
          "type": "thinking",
          "thinking": "Let me work through this step by step...\n1. First, I need to identify the key variables\n2. Then apply the formula...\n3. Finally verify the result",
          "signature": "EogBChYKCQjP8Ku7BhCAreUBEgxB9OppJQ6C6gFarT0audYlKkB5uF29FyIgx..."
        },
        "refusal": null
      }
    }
  ]
}
```

流式推理内容通过 delta.reasoning_content 逐块返回。

### 3.6 Token 使用统计（Usage）

| 字段                                                     | 类型      | 说明                  |
| ------------------------------------------------------ | ------- | ------------------- |
| `prompt_tokens`                                        | integer | 输入 token 数量         |
| `completion_tokens`                                    | integer | 输出 token 数量         |
| `total_tokens`                                         | integer | 总 token 数量          |
| `prompt_tokens_details.cached_tokens`                  | integer | 缓存命中 token（节省费用）    |
| `prompt_tokens_details.audio_tokens`                   | integer | 音频输入 token          |
| `completion_tokens_details.reasoning_tokens`           | integer | 推理链 token（o1/o3 系列） |
| `completion_tokens_details.audio_tokens`               | integer | 音频输出 token          |
| `completion_tokens_details.accepted_prediction_tokens` | integer | 预测编辑中接受的 token      |
| `completion_tokens_details.rejected_prediction_tokens` | integer | 预测编辑中拒绝的 token      |

### 3.7 结束原因（Finish Reason）

| 值                | 含义                            |
| ---------------- | ----------------------------- |
| `stop`           | 自然停止（遇到停止词或完成）                |
| `length`         | 达到 `max_completion_tokens` 限制 |
| `tool_calls`     | 模型决定调用工具                      |
| `content_filter` | 内容过滤触发（安全政策）                  |
| `refusal`        | 模型明确拒绝请求                      |

## 4. 错误响应格式

HTTP 状态码 >= 400 时返回：
```json
{
  "error": {
    "message": "You exceeded your current quota, please check your plan and billing details.",
    "type": "insufficient_quota",
    "param": null,
    "code": "insufficient_quota"
  }
}
```

常见错误类型：
| type                    | code                      | 说明       |
| ----------------------- | ------------------------- | -------- |
| `invalid_request_error` | `context_length_exceeded` | 上下文长度超限  |
| `invalid_request_error` | `invalid_api_key`         | API 密钥无效 |
| `rate_limit_error`      | `rate_limit_exceeded`     | 速率限制     |
| `authentication_error`  | -                         | 认证失败     |
| `server_error`          | -                         | 服务器内部错误  |

## 5. 模型系列差异对照表

| 功能/模型               | GPT-4/GPT-4o/GPT-4.1                   | o1/o3-mini/o4-mini            | gpt-5                         |
| ------------------- | -------------------------------------- | ----------------------------- | ----------------------------- |
| 系统角色                | `system`                               | `developer`                   | `developer`                   |
| 输出限制参数              | `max_tokens` / `max_completion_tokens` | **仅** `max_completion_tokens` | **仅** `max_completion_tokens` |
| 采样参数 (temperature等) | ✅ 支持                                   | ❌ 不支持                         | ⚠️ 部分支持                       |
| 推理参数                | -                                      | `reasoning_effort`            | `verbosity`                   |
| 推理内容输出              | ❌                                      | ✅ `reasoning_content`         | ✅ `reasoning_content`         |
| 流式传输                | ✅                                      | ✅                             | ✅                             |
| 工具调用                | ✅                                      | ✅                             | ✅                             |
| 结构化输出               | ✅                                      | ✅                             | ✅                             |

## 6. 完整对话示例

### 6.1 标准对话流程

Request：
```json
{
  "model": "gpt-4o-mini",
  "messages": [
    {"role": "system", "content": "You are a helpful assistant."},
    {"role": "user", "content": "What is the weather like?"}
  ],
  "temperature": 0.7
}
```

Response：
```json
{
  "id": "chatcmpl-123",
  "object": "chat.completion",
  "created": 1677652288,
  "model": "gpt-4o-mini",
  "choices": [{
    "index": 0,
    "message": {
      "role": "assistant",
      "content": "I don't have access to real-time weather data. Please check a weather service."
    },
    "finish_reason": "stop"
  }],
  "usage": {
    "prompt_tokens": 23,
    "completion_tokens": 15,
    "total_tokens": 38
  }
}
```

### 6.2 工具调用完整流程

#### Step 1 - 请求工具：

```json
// Request 包含 tools 定义
{
  "model": "gpt-4o-mini",
  "messages": [{"role": "user", "content": "天气怎么样？"}],
  "tools": [...],
  "tool_choice": "auto"
}

// Response 返回 tool_calls
{
  "choices": [{
    "message": {
      "role": "assistant",
      "content": null,
      "tool_calls": [{
        "id": "call_1",
        "type": "function",
        "function": {
          "name": "get_weather",
          "arguments": "{\"location\": \"当前位置\"}"
        }
      }]
    },
    "finish_reason": "tool_calls"
  }]
}
```

#### Step 2 - 返回工具结果：

```json
// 将 tool 结果加入 messages 数组
{
  "model": "gpt-4o-mini",
  "messages": [
    {"role": "user", "content": "天气怎么样？"},
    {"role": "assistant", "content": null, "tool_calls": [...]},
    {
      "role": "tool", 
      "tool_call_id": "call_1", 
      "content": "{\"temperature\": 25, \"condition\": \"sunny\"}"
    }
  ]
}
```

## 7. 版本兼容性说明

- max_tokens 弃用：自 2024 年底起，OpenAI 推荐使用 max_completion_tokens 替代 max_tokens，推理模型仅支持前者。
- 角色变更：o1/o3 及 gpt-5 系列模型使用 developer 角色替代 system 角色传递系统指令。
- 流式 Usage：需在请求中设置 stream_options: {"include_usage": true} 才能在流式响应末尾获取 token 统计。
- 严格模式：JSON Schema 结构化输出的 strict: true 仅支持 gpt-4o-2024-08-06 及更新模型。

文档版本: v2025.08
最后更新: 2025-08-01