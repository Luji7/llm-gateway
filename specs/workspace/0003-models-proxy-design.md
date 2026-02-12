# 0003 - Anthropic `/v1/models` 代理设计方案

**文档状态**: Draft
**版本**: v0.1
**目标**: 在现有 AI 网关中新增 Anthropic 格式的 `/v1/models` 代理能力，对接 OpenAI 兼容 `/v1/models`，并返回 Anthropic 规范结构。

---

## 1. 背景与目标

### 1.1 背景
当前网关支持 Anthropic `/v1/messages` 到 OpenAI 兼容转发。需要补齐 Anthropic `/v1/models` 端点，以便上游能查询模型列表。

### 1.2 目标
- 对外暴露 `GET /v1/models`（Anthropic 规范）
- 将下游 OpenAI `/v1/models` 转换为 Anthropic `/v1/models` 响应结构
- 支持自定义 `display_name` 映射
- 保持错误响应为 Anthropic 风格

### 1.3 非目标
- 不实现分页（Anthropic 规范不支持分页）
- 不实现模型路由策略（仅查询下游默认列表）

---

## 2. API 设计

### 2.1 Endpoint
- **方法**: `GET`
- **路径**: `/v1/models`
- **请求头**:
  - `x-api-key`（Anthropic 规范要求，网关用于上游校验/透传无需强制）
  - `anthropic-version`（可选校验，不影响下游）

### 2.2 响应（Anthropic 规范）
```json
{
  "data": [
    {
      "id": "claude-sonnet-4-20250514",
      "type": "model",
      "display_name": "Claude Sonnet 4",
      "created_at": "2025-05-14T00:00:00Z"
    }
  ]
}
```

---

## 3. 下游对接与映射规则

### 3.1 下游请求
- 下游为 OpenAI 兼容 `/v1/models`
- 认证：`Authorization: Bearer <OPENAI_API_KEY>`

### 3.2 字段映射
| OpenAI 字段 | Anthropic 字段 | 规则 |
| --- | --- | --- |
| `id` | `id` | 透传 |
| `object` | `type` | 固定转换为 `model` |
| `created` (unix 秒) | `created_at` (ISO 8601) | 转换为 UTC `YYYY-MM-DDTHH:mm:ssZ` |
| `owned_by` | - | 丢弃 |
| `display_name` | `display_name` | 通过映射表或默认规则补充 |

### 3.3 display_name 生成策略
优先级：
1. `MODEL_DISPLAY_MAP` 环境变量（JSON map）
2. 规则生成（基于 model id 分词）
3. 回退为原始 `id`

示例规则：
- `gpt-4o-mini` -> `GPT-4o Mini`
- `claude-sonnet-4-20250514` -> `Claude Sonnet 4`

---

## 4. 错误处理

### 4.1 错误映射
- 下游错误状态映射为 Anthropic error：
  - 401 -> `authentication_error`
  - 403 -> `permission_error`
  - 429 -> `rate_limit_error`
  - 500/503 -> `api_error`

### 4.2 错误响应格式
```json
{
  "type": "error",
  "error": {
    "type": "authentication_error",
    "message": "API key invalid"
  }
}
```

---

## 5. 配置项

- `OPENAI_BASE_URL`（已有）
- `OPENAI_API_KEY`（已有）
- `MODEL_DISPLAY_MAP`（新增，JSON 字符串）
  - 示例：`{"gpt-4o-mini":"GPT-4o Mini","kimi-k2.5":"Kimi K2.5"}`

---

## 6. 代码结构与模块

### 6.1 新增结构体
- `AnthropicModelsResponse`
- `AnthropicModel`
- `OpenAIModelsResponse`
- `OpenAIModel`

### 6.2 新增 handler
- `GET /v1/models` -> `handlers::get_models`

### 6.3 转换函数
- `openai_models_to_anthropic(resp: OpenAIModelsResponse) -> AnthropicModelsResponse`

---

## 7. 测试计划

### 7.1 单元测试
- OpenAI models -> Anthropic models 映射
- `created` 时间戳转换
- display_name 生成策略

### 7.2 集成测试
- mock 下游 `/v1/models` 返回
- 验证 Anthropic `/v1/models` 输出结构

---

## 8. 风险与注意事项

- 下游可能返回分页字段，需要忽略
- 某些模型 `created` 字段可能缺失（需回退为当前时间或 null）
- display_name 生成规则可能不准确，应优先映射表

