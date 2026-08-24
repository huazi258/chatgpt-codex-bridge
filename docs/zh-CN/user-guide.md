# 用户操作指南

这份文档解释当前 Conversation Relay V2 界面中各个功能和状态分别是什么意思。

## 一、先理解四个角色

| 角色 | 负责什么 |
| --- | --- |
| 用户 | 准备项目、选择工作目录、提供上下文、处理人工决策、最终验收。 |
| ChatGPT | 分析需求、规划工程任务、审查 Codex 结果、生成下一条 Codex Prompt。 |
| Codex | 在指定本地目录中读取代码并执行工程工作。 |
| Bridge | 传递消息、维护队列和状态、管理当前 Codex thread、执行预算和恢复规则。 |

Bridge 不是新的 Coding Agent。

它本身不替 ChatGPT 做规划，也不替 Codex 写代码。

## 二、界面结构

当前桌面界面主要分为：

- 左侧模块列表；
- ChatGPT 浏览器连接；
- 全局通道状态；
- 模块状态；
- Codex 通讯状态；
- ChatGPT 常驻对话；
- 验收面板；
- UNKNOWN 消息恢复；
- Codex thread 恢复。

## 三、ChatGPT 浏览器连接

Bridge 通过 Chrome 扩展连接一个已登录的 ChatGPT 标签页。

连接区域会显示：

- 本机地址；
- 一次性配对密钥；
- 当前连接详情；
- **刷新连接状态**。

配对密钥只对当前 Bridge 进程有效。

Bridge 重启以后，需要重新配对。

## 四、模块是什么？

模块是 Bridge 中最主要的工作单位。

你可以把它理解为：

> 一段连续的 AI 编程工作阶段。

模块里保存：

- 模块名称；
- Codex 工作目录；
- Codex thread；
- ChatGPT 消息历史；
- Codex relay cycle；
- 最大循环次数；
- 最大运行时间；
- 当前生命周期状态；
- 异常恢复状态。

一个模块可以完成很多个实际工程任务。

## 五、新建 Codex 对话

创建模块时选择：

```text
新建 Codex 对话
```

此时 Bridge 不会马上创建 Codex thread。

只有第一条合法 `CODEX_PROMPT` 到来后才真正创建。

这样做可以避免：

> 创建了模块，但实际上还没准备让 Codex 工作。

## 六、继续现有 Codex 对话

选择：

```text
继续现有 Codex 对话
```

然后：

1. 输入 Codex 工作目录；
2. 点击 **刷新对话**；
3. 选择候选 Codex thread；
4. 创建模块。

Bridge 不会自动接管外部仍然 active 的 thread。

如果一个 thread 当前正在别处执行，通常只能查看，不能直接选择继续。

## 七、手动聊天

点击：

```text
手动聊天
```

表示：

> 这条消息只是和 ChatGPT 正常交流。

适合：

- 补充背景；
- 讨论方案；
- 问问题；
- 检查 ChatGPT 的理解；
- 在真正开始自动化前准备上下文。

即使 ChatGPT 在手动回复里写出了：

```text
@@@CODEX_PROMPT@@@
```

Bridge 也不会执行。

## 八、发送自动化请求

点击：

```text
发送自动化请求
```

表示：

> 这条消息对应的 ChatGPT 回复可以参与自动化控制。

只有这种回复会解析控制块。

## 九、CODEX_PROMPT

格式：

```text
@@@CODEX_PROMPT@@@
<完整提示词>
@@@END_CODEX_PROMPT@@@
```

Bridge 会把中间内容原样发送给 Codex。

Bridge 不会追加：

- Git 指令；
- 测试指令；
- 分支规则；
- Commit 要求；
- 完成条件。

这些应该存在于：

- ChatGPT 生成的 Prompt；
- 或目标项目自己的 `AGENTS.md` 等项目说明里。

## 十、MODULE_DONE

格式：

```text
@@@MODULE_DONE@@@
```

表示：

> ChatGPT 判断当前模块已经可以交给用户验收。

模块会进入：

```text
WAITING_FOR_ACCEPTANCE
```

但此时还没有真正完成。

## 十一、验收模块

进入验收状态以后，你可以选择：

### 验收

表示你认可当前结果。

模块进入完成状态，并停止后续 ChatGPT / Codex 自动工作。

### 提交反馈并继续

如果你认为工作还没做好，可以输入反馈。

反馈会重新进入 ChatGPT 的自动化消息队列。

ChatGPT 可以根据反馈继续返回：

```text
CODEX_PROMPT
```

让 Codex 继续修改。

### 终止

如果你不想继续这个模块，可以终止。

## 十二、BLOCKED

格式：

```text
@@@BLOCKED@@@
<原因>
@@@END_BLOCKED@@@
```

表示 ChatGPT 判断：

> 当前必须由用户提供信息或做决定。

Bridge 会停止自动继续。

这类情况可能包括：

- 需求存在歧义；
- 需要选择实现方案；
- 需要确认危险操作；
- 缺少外部信息；
- ChatGPT 无法安全替用户做决定。

## 十三、Codex 通讯 Cycle

每一个合法的：

```text
CODEX_PROMPT
```

都会对应一个 Codex relay cycle。

一个 cycle 会经历类似：

```text
等待发送 Codex
        ↓
Codex turn 启动
        ↓
Codex 执行中
        ↓
Codex 返回最终文本
        ↓
结果排队发送给 ChatGPT
```

界面中的：

```text
已开始循环
```

只有 Codex turn 真正启动后才增加。

## 十四、运行预算

每个模块有两个预算。

### 最大自动循环次数

控制最多启动多少个 Codex turn。

### 最大运行时间

从模块第一次 Codex turn 启动以后开始计算。

这些预算只是安全边界。

它们不会告诉 ChatGPT 怎么规划任务。

## 十五、消息状态

Bridge 会显示真实的消息送达状态。

常见状态包括：

### QUEUED

已经保存，等待发送。

### SENT

已经提交给传输层。

### DELIVERED

已经确认成功送达。

### FAILED

发送失败。

### UNKNOWN

无法确认消息究竟有没有成功送达。

## 十六、UNKNOWN 消息

`UNKNOWN` 是 Bridge 一个非常重要的安全状态。

例如：

1. Bridge 正在发送一条 ChatGPT 消息；
2. 应用突然退出；
3. 重启后 Bridge 不知道：
   - 消息其实已经到 ChatGPT；
   - 还是根本没发送出去。

如果自动重发，可能导致同一请求执行两遍。

因此 Bridge 不会猜。

你必须自己选择：

### 明确重发这条消息

确定再次发送。

### 不重发并继续

记录这条消息不再发送。

所有 UNKNOWN 消息都处理完以后，队列才会继续。

## 十七、RECOVERY_REQUIRED

如果 Bridge 无法确认：

- Codex thread 是否仍然安全可继续；
- thread/resume 是否真的成功；
- 某个具有副作用的操作到底执行了没有；

模块可能进入：

```text
RECOVERY_REQUIRED
```

此时应该使用界面出现的 Codex 恢复面板。

不要在外部盲目重复：

- thread/start；
- thread/resume；
- Codex App Server 操作。

因为可能产生重复 thread 或所有权冲突。

## 十八、终止模块

用户可以请求终止模块。

如果 Codex 当前没有运行，模块可以直接进入终止流程。

如果 Codex 已经有一个 turn 正在运行：

> Bridge 会允许这个 turn 正常结束，但不会再把结果继续用于下一轮自动化。

这是为了避免在执行过程中强行破坏 Codex 状态。

## 十九、协议重试

如果自动化请求对应的 ChatGPT 回复没有合法控制块：

Bridge 会使用模块配置的重试模板再尝试一次。

如果第二次仍然无效：

> 自动化停止，并要求用户处理。

手动聊天不会进入这套协议重试。

## 二十、终态

### COMPLETED

用户已经明确验收。

模块只保留历史，不再发送消息或启动 Codex。

### STOPPED

模块已经终止。

历史仍然保留。

## 推荐工作方式

建议：

1. 先用普通 ChatGPT 对话建立项目上下文；
2. 创建一个明确的 Bridge 模块；
3. 使用自动化请求启动 ChatGPT → Codex 工作；
4. 让 ChatGPT 根据 Codex 结果继续审查；
5. 遇到 `BLOCKED` 时由用户做决定；
6. 遇到 `UNKNOWN` 时由用户处理送达不确定性；
7. 遇到 `RECOVERY_REQUIRED` 时按恢复面板操作；
8. 最后在 `MODULE_DONE` 后进行人工验收。
