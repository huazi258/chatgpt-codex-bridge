# ChatGPT Codex Bridge

**ChatGPT 负责思考，Codex 负责执行，Bridge 负责把它们连接起来。**

[English](README.md) | **简体中文**

ChatGPT Codex Bridge 是一个运行在本地的桌面中间件，用来把一个 ChatGPT 对话与一个 Codex 对话线程连接起来，形成一个可控的 AI 编程工作流。

在这个工作流里：

- **ChatGPT** 负责理解需求、规划下一步、审查 Codex 的执行结果；
- **Codex** 负责在指定的本地项目目录中读取代码、修改代码和执行工程任务；
- **Bridge** 负责在两者之间可靠传递消息、维护队列和状态、控制运行预算，并在需要人工决策时暂停；
- **用户** 负责准备项目、提供上下文，以及最终验收或终止模块。

> 本项目是独立的开源项目，与 OpenAI 无隶属或官方背书关系。

## 这个项目解决什么问题？

如果你平时会这样工作：

1. 在 ChatGPT 里讨论需求和实现方案；
2. 让 ChatGPT 写一段给 Codex 的提示词；
3. 手动复制到 Codex；
4. 等 Codex 执行完；
5. 再把 Codex 的结果复制回 ChatGPT；
6. 让 ChatGPT 继续审查和安排下一步；

那么 Bridge 做的事情，就是把这段“人工来回复制”的过程变成一个**可见、可恢复、有状态的本地传话链路**。

```text
你
 │
 ▼
ChatGPT ── 分析 / 规划 / 审查
 │
 │  CODEX_PROMPT
 ▼
Bridge
 │
 ▼
Codex ── 在本地项目中执行
 │
 │  执行结果
 ▼
Bridge
 │
 ▼
ChatGPT ── 审查结果 / 决定下一步
 │
 ├─ CODEX_PROMPT → 继续让 Codex 工作
 ├─ BLOCKED      → 请求用户介入
 └─ MODULE_DONE  → 请求用户验收
```

Bridge **不会**替 ChatGPT 决定怎么实现功能，也不会替 Codex 管理 Git 策略、测试规则或工程决策。

它只负责：

- 消息传递；
- 队列顺序；
- ChatGPT / Codex 连接状态；
- 模块生命周期；
- Codex 对话线程；
- 最大自动循环次数；
- 最大运行时间；
- 异常恢复；
- 人工验收。

## 核心概念

### 模块（Module）

一个模块代表一次可见的工作阶段。

每个模块绑定：

- 一个 ChatGPT 对话；
- 一个 Codex 工作目录；
- 一个新建或继续使用的 Codex 对话；
- 最大自动循环次数；
- 最大运行时间。

一个模块可以包含很多次 ChatGPT ↔ Codex 往返，不等于一次 Codex 请求。

### 手动聊天

使用 **手动聊天** 时，你只是通过 Bridge 和 ChatGPT 正常交流。

ChatGPT 的回复会显示在 Bridge 中，但**不会触发 Codex 自动执行**。

### 自动化请求

使用 **发送自动化请求** 时，ChatGPT 对应的下一条回复才有资格被 Bridge 解析成自动化控制指令。

## 环境要求

当前版本默认面向本地开发者使用。

你需要：

- Windows 环境；
- Node.js 22.11 或更高版本；
- Rust stable 工具链；
- Cargo；
- 本地可用的 Codex；
- Google Chrome；
- 已登录的 ChatGPT；
- Git。

## 快速开始

### 1. 克隆仓库

```powershell
git clone https://github.com/huazi258/chatgpt-codex-bridge.git
cd chatgpt-codex-bridge
```

### 2. 安装依赖

```powershell
npm install --registry=https://registry.npmjs.org
```

### 3. 启动 Bridge

```powershell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
npm run tauri -- dev
```

启动成功后，会打开一个中文桌面界面。

在界面的 **ChatGPT 浏览器连接** 区域可以看到：

- 本机地址；
- 一次性配对密钥；
- 当前 ChatGPT 连接状态。

### 4. 加载 Chrome 扩展

打开：

```text
chrome://extensions
```

然后：

1. 开启 **开发者模式**；
2. 点击 **加载已解压的扩展程序**；
3. 选择仓库中的：

```text
spikes/chatgpt-extension
```

4. 打开你准备给 Bridge 使用的 `https://chatgpt.com` 对话；
5. 刷新一次 ChatGPT 页面。

### 5. 配对 ChatGPT

1. 保持 Bridge 桌面程序运行；
2. 在 Bridge 中复制 **一次性配对密钥**；
3. 回到目标 ChatGPT 标签页；
4. 打开 Bridge Chrome 扩展；
5. 粘贴密钥；
6. 配对当前标签页。

成功后 Bridge 左侧应显示：

```text
ChatGPT 已连接
```

> Bridge 每次重启都会生成新的配对密钥，旧密钥会失效。

### 6. 新建模块

点击：

```text
新建模块
```

然后填写：

- 模块名称；
- Codex 工作目录；
- Codex 对话方式；
- 最大自动循环次数；
- 模块最长运行时间。

Codex 对话有两种选择：

#### 新建 Codex 对话

收到第一份有效的 `CODEX_PROMPT` 后才真正创建 Codex 对话。

#### 继续现有 Codex 对话

输入工作目录后：

1. 选择 **继续现有 Codex 对话**；
2. 点击 **刷新对话**；
3. 选择一个可继续的 Codex 对话；
4. 创建模块。

Bridge 不会强制接管当前正在其他地方运行的 Codex 对话。

### 7. 开始自动化

在模块中选择：

```text
发送自动化请求
```

只有这种消息对应的 ChatGPT 回复会参与自动化控制。

ChatGPT 的有效回复必须以以下三个控制块之一结尾。

#### 让 Codex 执行

```text
@@@CODEX_PROMPT@@@
<完整且原样发送给 Codex 的提示词>
@@@END_CODEX_PROMPT@@@
```

#### 请求模块验收

```text
@@@MODULE_DONE@@@
```

#### 请求用户介入

```text
@@@BLOCKED@@@
<为什么需要用户，以及需要用户提供什么信息或决定>
@@@END_BLOCKED@@@
```

Bridge 会把 `CODEX_PROMPT` 中间的内容**原样发送给 Codex**。

Codex 执行完成后，结果会自动进入 ChatGPT 消息队列，再由 ChatGPT 判断下一步。

## `MODULE_DONE` 是什么意思？

`MODULE_DONE` 不代表 Bridge 会自动宣布整个模块完成。

它的意思是：

> ChatGPT 认为当前工作已经可以交给用户验收。

此时模块进入：

```text
WAITING_FOR_ACCEPTANCE
```

你可以：

- 验收完成；
- 提交反馈，让 ChatGPT 继续；
- 终止模块。

只有用户明确验收后，模块才真正进入完成状态。

## `BLOCKED` 是什么意思？

当 ChatGPT 判断当前工作必须由用户做决定时，可以返回：

```text
@@@BLOCKED@@@
...
@@@END_BLOCKED@@@
```

Bridge 会停止自动继续，把原因显示给用户。

## `UNKNOWN` 是什么意思？

如果发生：

- Bridge 重启；
- 网络或页面连接中断；
- 某条消息到底有没有送到 ChatGPT 无法确认；

Bridge 不会自动重发。

因为自动重发有可能导致同一条消息执行两次。

这时消息状态会变成：

```text
UNKNOWN
```

你必须明确选择：

- **明确重发这条消息**
- **不重发并继续**

处理完所有 `UNKNOWN` 消息之后，消息队列才会继续。

## 推荐使用方式

比较推荐的使用流程是：

1. 先在 ChatGPT 中正常讨论项目和需求；
2. 准备好目标项目目录以及 `AGENTS.md` 等项目说明；
3. 启动 Bridge；
4. 配对当前 ChatGPT 对话；
5. 创建一个工作模块；
6. 使用自动化请求让 ChatGPT开始规划；
7. ChatGPT 生成 `CODEX_PROMPT`；
8. Codex 执行；
9. 结果自动返回 ChatGPT；
10. ChatGPT 继续审查；
11. 重复以上过程；
12. ChatGPT 返回 `MODULE_DONE`；
13. 用户进行最终验收。

## 文档

第一次使用，建议按这个顺序阅读：

- [快速上手](docs/zh-CN/getting-started.md)
- [用户操作指南](docs/zh-CN/user-guide.md)
- [故障排查](docs/zh-CN/troubleshooting.md)
- [中文文档导航](docs/zh-CN/README.md)

开发和架构资料：

- [本地开发](docs/development/local-development.md)
- [Conversation Relay V2 决策](docs/decisions/004-conversation-relay-v2.md)
- [架构文档](docs/architecture/)
- [协议文档](docs/protocols/)
- [可靠性设计](docs/reliability/)
- [安全设计](docs/security/)

## 当前限制

当前版本依赖：

- 本地 Chrome 扩展；
- `https://chatgpt.com` 页面结构；
- ChatGPT 页面 DOM selector；
- 本地 Codex App Server。

如果 ChatGPT 前端页面发生较大变化，Chrome 扩展可能需要同步更新。

Bridge 对不确定消息和 Codex 对话所有权采取保守策略：

> 无法确认时，停止并让用户明确处理，而不是自动猜测。

这也是当前产品设计的一部分。
