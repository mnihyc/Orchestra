---
screens: [/resources/providers, /thread/$threadId, /usage]
keywords: [网络搜索, 原生搜索, Google, 搜索费用, 缓存, 实时, DeepSeek]
---

# 提供方网络搜索

提供方可以向代理、摘要和目标检查提供模型的原生网络搜索。提供方管理者决定允许哪些模式，以及新的使用默认选择什么。来源列出支持的模式不会自动启用它。

在提供方的“模型”页面启用支持的模式并选择默认值。线程随后在“参数”中选择提供方允许的模式。“关闭”移除托管搜索工具；“缓存”在协议支持时仅使用索引内容，不实时读取外部页面；“实时”允许模型按需搜索公开网络。默认值仍可覆盖；固定值显示其来源。

协议兼容不代表每个模型都支持搜索。DeepSeek 文档明确说明 Responses 接口的 Pro 模型支持服务器端搜索；忽略搜索选项的模型不能算作实际运行了搜索。Google 的 OpenAI 兼容文档仅在 Images 端点列出搜索工具，没有说明 Chat 的搜索映射。因此 Google 搜索 grounding 需要原生协议或声明相应映射的翻译器。

## 配置托管工具

先在提供方模型上启用模式。Responses 端点的请求默认值示例：

```json
{"web_search":{"mode":"live","search_context_size":"low","filters":{"allowed_domains":["example.com"]}}}
```

Messages 使用其自身的选项：

```json
{"web_search":{"mode":"live","max_uses":3,"allowed_domains":["example.com"]}}
```

允许列表与屏蔽列表不能同时使用。每次使用的“关闭”选择覆盖默认值。端点默认值只配置代理已选择的工具，不会启用运行程序未列出的工具。摘要和目标检查通过相同的提供方模型限制解析其选择。

## 设置搜索价格

模型费率和费率表支持“每次网络搜索的额度”（`web_search_per_call`）。它与令牌费率使用相同的额度单位。如果额度以美元计，文档中的基本搜索费为 OpenAI 和 Anthropic 的 `0.01`，以及 DeepSeek 每次成功搜索的 `0.00075`。搜索费用不乘以令牌的服务档位倍率；搜索内容本身仍可能产生模型令牌费用。

每个请求保留来源实际报告的搜索次数。一次可见的 WebSearch 工具调用可能包含多次后端搜索，不能直接当作计费次数。缺少次数或价格时，完整费用为未知。重复的累计报告不会增加使用量。这些是按配置计算的估算，不是账单。

原生网络搜索查找公开网页。Orch 的 `search_shared` 查找代理有权读取的共享文档、聊天和线程，两者独立。

来源：[OpenAI 网络搜索](https://developers.openai.com/api/docs/guides/tools-web-search)、[Anthropic 网络搜索](https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool)、[DeepSeek Responses](https://api-docs.deepseek.com/guides/responses_api/)、[Gemini grounding](https://ai.google.dev/gemini-api/docs/google-search)。
