# 中文文档导航

[English documentation](../README.md) | **简体中文**

如果你刚刚从 GitHub 克隆 ChatGPT Codex Bridge，建议不要直接阅读 architecture / protocol / decision 文档。

先按照用户视角阅读。

## 第一次使用

| 文档 | 作用 |
| --- | --- |
| [快速上手](getting-started.md) | 从 clone 仓库一直到第一次 ChatGPT → Codex → ChatGPT 循环。 |
| [用户操作指南](user-guide.md) | 解释模块、消息类型、Codex thread、运行预算、验收和恢复。 |
| [故障排查](troubleshooting.md) | 处理启动、Chrome 配对、Codex、协议、UNKNOWN 和恢复问题。 |

GitHub 首页可以查看：

[中文 README](../../README.zh-CN.md)

## 如果你准备修改 Bridge

请继续阅读：

- [`AGENTS.md`](../../AGENTS.md)
- [本地开发](../development/local-development.md)
- [Conversation Relay V2](../decisions/004-conversation-relay-v2.md)

## 如果你准备理解内部设计

主要资料包括：

- `architecture/`
- `decisions/`
- `protocols/`
- `product-specs/`
- `reliability/`
- `security/`
- `superpowers/specs/`

其中：

> 最新 accepted decision 的优先级高于与其冲突的历史 V1 文档。

当前最重要的产品合同是：

[004 — Conversation relay V2](../decisions/004-conversation-relay-v2.md)

## 文档阅读顺序建议

### 普通使用者

```text
README.zh-CN.md
        ↓
快速上手
        ↓
用户操作指南
        ↓
故障排查
```

### Contributor

```text
AGENTS.md
        ↓
Local Development
        ↓
当前实现
        ↓
V2 Decision
```

### Maintainer

```text
最新 accepted decisions
        ↓
architecture / protocols
        ↓
reliability / security
        ↓
historical specs
```
