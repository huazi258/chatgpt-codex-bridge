# 快速上手

这份文档的目标只有一个：

> 从 GitHub 克隆仓库开始，一直到完成第一次 ChatGPT → Codex → ChatGPT 自动传话。

如果你已经能运行 Bridge，只是不清楚界面、模块和状态分别是什么意思，请看 [用户操作指南](user-guide.md)。

## 1. 准备环境

建议先确认以下工具已经安装：

- Windows；
- Node.js 22.11 或更高版本；
- Rust stable；
- Cargo；
- Codex；
- Google Chrome；
- Git；
- 已登录的 ChatGPT。

可以在 PowerShell 中执行：

```powershell
node --version
npm --version
cargo --version
codex --version
git --version
```

如果某一项无法识别，先解决对应工具的安装或 `PATH` 问题。

## 2. 克隆仓库

```powershell
git clone https://github.com/huazi258/chatgpt-codex-bridge.git
cd chatgpt-codex-bridge
```

## 3. 安装依赖

```powershell
npm install --registry=https://registry.npmjs.org
```

显式使用 npm 官方 registry，是因为某些镜像源可能缺少需要的 `@tauri-apps/*` 包。

## 4. 启动 Bridge

```powershell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
npm run tauri -- dev
```

正常情况下会启动一个中文桌面窗口。

你应该能看到：

```text
ChatGPT 浏览器连接
```

以及：

- 本机地址；
- 一次性配对密钥；
- 当前连接状态。

## 5. 加载 Chrome 扩展

Bridge 当前使用仓库内的未打包 Chrome 扩展。

打开：

```text
chrome://extensions
```

然后：

1. 打开 **开发者模式**；
2. 点击 **加载已解压的扩展程序**；
3. 选择：

```text
spikes/chatgpt-extension
```

4. 打开一个已经登录的 ChatGPT 对话；
5. 刷新一次页面。

扩展只作用于：

```text
https://chatgpt.com/*
```

## 6. 配对 ChatGPT 对话

回到 Bridge，复制：

```text
一次性配对密钥
```

然后：

1. 切换到目标 ChatGPT 标签页；
2. 打开 Bridge Chrome 扩展；
3. 粘贴密钥；
4. 配对当前标签页；
5. 回到 Bridge。

成功后左侧状态应该变成：

```text
ChatGPT 已连接
```

如果重启 Bridge，需要重新配对。

## 7. 准备 Codex 要操作的项目

Bridge 不替你选择仓库，也不会替你决定 Git 分支。

创建模块之前，请确认：

- 目标目录真实存在；
- Codex 可以正常进入该目录工作；
- 项目的 `AGENTS.md` 等指令已经准备好；
- Git 状态是你预期的状态；
- 不需要的敏感信息不要写入 Prompt。

Bridge 中的：

```text
Codex 工作目录
```

只是告诉 Codex：

> 这一次应该在哪个目录里工作。

## 8. 新建模块

点击：

```text
新建模块
```

填写以下内容。

### 模块名称

例如：

```text
README 中文使用文档
```

模块名称只是方便你自己识别。

### Codex 工作目录

输入目标项目的绝对路径，例如：

```text
G:\projects\my-project
```

### Codex 对话

有两种模式。

#### 新建 Codex 对话

适合开始一段新的工作。

创建模块时不会立刻创建 Codex thread。

只有收到第一条合法的 `CODEX_PROMPT` 后，Bridge 才会启动 App Server 并创建 Codex 对话。

#### 继续现有 Codex 对话

适合继续之前已经存在的 Codex thread。

操作：

1. 选择 **继续现有 Codex 对话**；
2. 点击 **刷新对话**；
3. 从当前工作目录对应的候选列表里选择一个；
4. 创建模块。

如果你修改了工作目录，需要重新刷新 Codex 对话列表。

Bridge 不会接管当前正在外部运行的 active thread。

## 9. 设置运行预算

界面默认值当前是：

```text
最大自动循环次数：12
模块最长时间：240 分钟
```

自动循环次数不是 ChatGPT 消息数量。

只有真正启动了一次 Codex turn，才会消耗一个 cycle。

## 10. 理解自动化控制块

只有：

```text
发送自动化请求
```

对应的 ChatGPT 回复才会被解析。

有效回复必须以一个且只能一个控制块结尾。

### CODEX_PROMPT

```text
@@@CODEX_PROMPT@@@
<发送给 Codex 的完整提示词>
@@@END_CODEX_PROMPT@@@
```

Bridge 不会修改中间的提示词。

### MODULE_DONE

```text
@@@MODULE_DONE@@@
```

表示 ChatGPT 认为工作已经可以让用户验收。

### BLOCKED

```text
@@@BLOCKED@@@
<为什么需要用户，以及需要用户做什么>
@@@END_BLOCKED@@@
```

表示必须暂停自动化并请求用户介入。

## 11. 发送第一条自动化请求

在模块中选择：

```text
发送自动化请求
```

你可以给 ChatGPT 类似这样的要求：

```text
请根据当前项目上下文判断下一步。

如果需要 Codex 执行，请在回复最后输出一个 CODEX_PROMPT 控制块；
如果需要我提供信息或决定，请输出 BLOCKED；
如果当前模块已经可以验收，请输出 MODULE_DONE。
```

Bridge 不负责替 ChatGPT 写工程方案。

具体让 Codex 做什么，应该由 ChatGPT 根据当前项目上下文决定。

## 12. 观察第一次完整传话

如果 ChatGPT 返回：

```text
@@@CODEX_PROMPT@@@
...
@@@END_CODEX_PROMPT@@@
```

Bridge 会：

1. 保存这次 relay cycle；
2. 启动或继续目标 Codex 对话；
3. 启动 Codex turn；
4. Codex 在指定工作目录执行；
5. 保存 Codex 最终回复；
6. 把 Codex 结果重新送回 ChatGPT；
7. 等待 ChatGPT 决定下一步。

你应该能在 Bridge 中看到：

- ChatGPT 消息历史；
- Codex 通讯状态；
- 已开始循环数；
- 模块当前状态；
- 队列状态。

## 13. 第一次成功的判断标准

如果以下链路完整跑通，就说明基本环境已经成功：

```text
ChatGPT 已连接
        ↓
创建模块
        ↓
发送自动化请求
        ↓
ChatGPT 返回 CODEX_PROMPT
        ↓
Codex 开始工作
        ↓
Codex 返回结果
        ↓
结果进入 ChatGPT
        ↓
ChatGPT 继续给出下一步
```

最终 ChatGPT 可以：

- 再返回一个 `CODEX_PROMPT`；
- 返回 `BLOCKED`；
- 返回 `MODULE_DONE`。

## 14. MODULE_DONE 后怎么办？

收到：

```text
@@@MODULE_DONE@@@
```

之后，模块会进入：

```text
WAITING_FOR_ACCEPTANCE
```

此时你可以：

- 验收；
- 填写反馈，让 ChatGPT 继续处理；
- 终止。

只有你明确点击验收，模块才真正完成。

## 下一步

继续阅读：

- [用户操作指南](user-guide.md)
- [故障排查](troubleshooting.md)
