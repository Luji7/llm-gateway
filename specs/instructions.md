# Instructions

## project alpha 需求和设计文档

构建一个简单的，AI网关代理服务。它使用 Rust语言实现的Rest服务，提供Anthropic格式消息到OpenAI兼容格式消息的转发功能。

以下两篇消息格式文档可供参考：
- ./specs/anthropic消息格式.md 定义了anthropic格式的消息格式和约束。
- ./specs/openai兼容格式.md 定义了openai格式的消息格式和约束。

按照这个想法，帮我生成详细的需求和设计文档，放在./specs/workspace/0001-spec.md 文件中，输出为中文。

## implementation plan

按照 ./specs/workspace/0001-spec.md 中的需求和设计文档，生成一个详细的实现计划，放在 ./specs/workspace/0002-implementation-plan.md 文件中，输出为中文。

## phased implementation

按照 ./specs/workspace/0002-implementation-plan.md 完整实现这个项目的 phased 1 - 3 代码。

## new feature

增加一个新特性，支持anthropic格式的 /models Endpoint 代理。

以下是/models端口的消息结构说明可供参考：
- ./specs/anthropic models端点消息格式.md

./specs/openai兼容格式.md 定义了openai格式的消息格式和约束

## 新特性：anthropic消息转发

参考 ./specs/anthropic消息格式.md 的接口格式定义，深入思考如何在当前项目中增加支持anthropic格式消息的直接转发。用户可以在配置文件中控制是直接anthropic格式请求消息转发或是anthropic格式转换openai兼容格式转发（当前项目已支持）。输出详细的方案计划，不要直接修改。

方案计划修改建议：
- 直接转发作为默认配置

参考 ./specs/workspace/0004-passthrough-plan.md 的方案计划文档，考虑详细的实现步骤计划，并开始实施。