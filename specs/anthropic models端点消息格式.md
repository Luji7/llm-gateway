# Anthropic `/v1/models` Endpoint 接口规范

## 1. 接口基本信息

| 属性 | 说明 |
|------|------|
| **端点路径** | `/v1/models` |
| **HTTP 方法** | `GET` |
| **功能描述** | 获取当前账户可用模型列表，包含模型 ID、显示名称及创建时间 |
| **基础域名** | `api.anthropic.com` |

---

## 2. 请求消息格式

### 2.1 协议与路径
- **协议**: HTTPS
- **完整 URL**: `https://api.anthropic.com/v1/models`
- **HTTP 版本**: HTTP/1.1 或 HTTP/2

### 2.2 请求头（Request Headers）

| 字段名 | 必填 | 数据类型 | 字段说明 |
|--------|------|----------|----------|
| `x-api-key` | 是 | string | API 认证密钥，**直接传入原始密钥值，不需要 `Bearer` 前缀** |
| `anthropic-version` | 是 | string | API 版本标识，格式通常为 `YYYY-MM-DD`，例如 `2023-06-01` |
| `Content-Type` | 否 | string | 客户端可省略，如指定则应为 `application/json` |

### 2.3 查询参数（Query Parameters）

当前版本**不支持**分页查询参数（如 `limit`, `after`, `before` 等）。接口将一次性返回账户有权访问的全部模型列表，客户端需自行实现本地过滤或缓存。

---

## 3. 响应消息格式

### 3.1 成功响应（HTTP 200 OK）

**响应头**:
- `Content-Type`: `application/json`
- `anthropic-ratelimit-requests-remaining`: 剩余请求配额（如适用）

**响应体结构**:

| 字段名 | 数据类型 | 字段说明 |
|--------|----------|----------|
| `data` | array | 模型对象数组，按创建时间倒序排列 |

**Model Object 结构**（`data` 数组元素类型）:

| 字段名 | 数据类型 | 可空 | 字段说明 |
|--------|----------|------|----------|
| `id` | string | 否 | 模型唯一标识符，用于后续 API 请求的 `model` 参数赋值 |
| `type` | string | 否 | 对象类型，固定枚举值为 `"model"` |
| `display_name` | string | 否 | 人类可读的模型显示名称，用于界面展示 |
| `created_at` | string | 否 | 模型创建时间，ISO 8601 UTC 时间格式（`YYYY-MM-DDTHH:mm:ssZ`）|

### 3.2 错误响应

**HTTP 状态码与错误类型映射**:

| HTTP 状态码 | 错误类型 (error.type) | 触发场景 |
|-------------|---------------------|----------|
| 401 | `authentication_error` | `x-api-key` 缺失、格式错误或密钥无效 |
| 403 | `permission_error` | 密钥有效但无权访问模型列表（如组织权限限制）|
| 429 | `rate_limit_error` | 单位时间内请求次数超过账户配额限制 |
| 500 | `api_error` | Anthropic 服务端内部异常 |
| 503 | `api_error` | 服务暂时不可用 |

**错误响应体结构**:

| 字段名 | 数据类型 | 字段说明 |
|--------|----------|----------|
| `type` | string | 固定值为 `"error"`，表示当前为错误响应 |
| `error` | object | 错误详情对象 |
| `error.type` | string | 错误类型枚举，取值见上表 |
| `error.message` | string | 人类可读的错误描述文本 |

---

## 4. 字段取值规范

### 4.1 模型 ID 命名规则
- **格式**: `{系列}-{变体}-{版本}-{日期}`
- **字符集**: 小写字母、数字、连字符 `-`
- **示例**: `claude-sonnet-4-20250514`、`claude-haiku-4-5-20251001`

### 4.2 时间戳格式
- **标准**: ISO 8601 扩展格式
- **时区**: 强制 UTC，以 `Z` 后缀标识
- **精度**: 秒级，格式模板 `YYYY-MM-DDTHH:mm:ssZ`

### 4.3 枚举值定义

**`type` 字段枚举**:
- 正常响应对象: `"model"`
- 错误响应对象: `"error"`

**`error.type` 字段枚举**:
- `authentication_error`: 认证失败
- `permission_error`: 权限不足
- `rate_limit_error`: 请求限流
- `api_error`: 服务端错误

---

## 5. 与 OpenAI `/v1/models` 格式差异

| 对比维度 | Anthropic 规范 | OpenAI 规范 |
|----------|---------------|-------------|
| **认证机制** | Header: `x-api-key: &lt;原始密钥&gt;` | Header: `Authorization: Bearer &lt;密钥&gt;` |
| **对象类型标识** | 字段名: `type`，值: `"model"` | 字段名: `object`，值: `"model"` |
| **时间戳字段** | 字段名: `created_at`，ISO 8601 字符串 | 字段名: `created`，Unix 秒级整数 |
| **展示名称** | 提供 `display_name` 字段 | 不包含该字段 |
| **组织信息** | 不包含 | 可选字段 `owned_by` |
| **分页机制** | 不支持分页，全量返回 | 支持分页，含 `has_more`, `first_id`, `last_id` 字段 |

---

## 6. 网关代理适配要点

当网关代理此端点时，需实现以下协议转换：

- **请求头识别**: 检测 `x-api-key` 头部（而非 `Authorization`）以识别 Anthropic 格式请求
- **字段映射**: 将 OpenAI 风格的 `object` 映射为 `type`，Unix 时间戳 `created` 转换为 ISO 8601 格式 `created_at`
- **字段增补**: 为模型数据补充 `display_name`（可基于模型 ID 映射或透传上游配置）
- **分页处理**: 如后端为 OpenAI，需拉取全量列表后屏蔽分页字段，以内联数组形式返回