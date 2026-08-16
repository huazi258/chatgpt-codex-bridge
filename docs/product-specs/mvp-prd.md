# ChatGPT × Codex 工作流中间件 MVP

> Historical V1 contract. The active product contract is [Decision 004 — Conversation relay V2](../decisions/004-conversation-relay-v2.md); it supersedes this document where they conflict.

## 1. 目标

消除人工在 ChatGPT 与 Codex 之间复制、粘贴、等待和转述的工作。系统在本机运行，自动编排一个受控的闭环：ChatGPT 负责规划、Review 和决策；Codex 负责在仓库中执行；中间件负责传递上下文、追踪状态、执行预算和在必要时暂停。

## 2. MVP 范围

- 一次只运行一个模块、一个仓库、一个 ChatGPT 会话。
- ChatGPT 在专用 Chrome Profile 的 `chatgpt.com` 标签页中运行，由本地浏览器扩展读取回复、写入消息。
- Codex 经 App Server 运行；不驱动、不占用 Codex 桌面应用窗口。
- 每一轮 Codex 修改代码后必须运行约定测试、提交并推送到启动模块时选定的分支。
- 只将 Codex 的文字完成摘要和仓库提交信息发回 ChatGPT；ChatGPT 通过仓库地址检查完整 diff。
- 支持模块轮次上限、模块最长运行时间和全局运行时间上限。
- 正常完成或到达预算后，系统在完成当前轮后暂停并等待验收。
- 失败、歧义、需要用户输入或协议无法解析时立即暂停。

不在 MVP 范围内：多模块/多仓库并行、自动合并分支、自动解决失败、操作 Codex 桌面窗口、移动端控制。

## 3. 角色与职责

| 角色 | 职责 |
| --- | --- |
| 用户 | 启动模块；在暂停点验收、继续、终止或要求重新规划。 |
| ChatGPT | 拆解任务、Review 远程仓库、决定下一轮或模块完成。 |
| 中间件 | 解析协议、转发上下文、驱动 App Server、计时、执行预算和通知。 |
| Codex | 阅读任务上下文、修改代码、测试、提交、推送并输出完成摘要。 |

## 4. 用户流程

1. 用户打开中间件，选择仓库目录、目标分支、ChatGPT 专用标签页和预算。
2. 用户点击“启动模块”。中间件向 ChatGPT 注入编排协议和模块启动消息。
3. ChatGPT 返回 `NEXT_TASK`，其中包含完整的 Codex 任务说明与验收标准。
4. 中间件将任务交给 Codex App Server，持续接收事件但默认只显示简洁状态。
5. Codex 完成任务、测试、提交、推送后，中间件取得其文字摘要与 commit SHA。
6. 中间件将摘要、分支、commit SHA 和仓库地址发送给 ChatGPT 请求 Review。
7. ChatGPT 返回下一任务、模块完成或暂停状态；中间件据此继续或暂停。
8. 用户在暂停卡片中选择“验收通过 / 继续 / 终止 / 重新规划”。

## 5. 状态机

```text
IDLE
  -> STARTING_MODULE
  -> WAITING_FOR_CHATGPT_PLAN
  -> STARTING_CODEX_TURN
  -> CODEX_RUNNING
  -> WAITING_FOR_CHATGPT_REVIEW
  -> STARTING_CODEX_TURN | PAUSED_FOR_ACCEPTANCE | BLOCKED | COMPLETED | STOPPED
```

- 到达轮次或时间预算时，设置 `pauseAfterCurrentTurn=true`；不打断正在运行的 Codex 回合。
- 任何 `BLOCKED`、App Server 错误、测试/提交/推送失败、无法解析协议或 Codex 请求澄清，均转入 `BLOCKED`。
- `PAUSED_FOR_ACCEPTANCE` 仅由用户操作离开。

## 6. ChatGPT 编排协议

中间件在启动时发送协议，要求 ChatGPT 的每条自动化回复末尾包含且只包含一个 JSON 代码块。自然语言说明保留给用户阅读；中间件只解析 JSON。

```json
{
  "state": "NEXT_TASK | MODULE_DONE | PAUSE | BLOCKED",
  "module": "模块名称",
  "reason": "简短、面向用户的原因",
  "codex_prompt": "仅 NEXT_TASK 时必填，完整可执行任务",
  "acceptance_criteria": ["验收条件"],
  "review_scope": "commit SHA 或分支",
  "requires_user_input": false
}
```

规则：

- `NEXT_TASK` 必须包含 `codex_prompt` 和至少一条 `acceptance_criteria`。
- `MODULE_DONE` 表示模块通过 ChatGPT Review；中间件转入验收暂停，而不是继续创建任务。
- `PAUSE` 和 `BLOCKED` 均停止自动链，`reason` 必填。
- 缺字段、多个 JSON 块或非合法 JSON 均视为协议错误并暂停。

## 7. Codex 任务包装

中间件为每一轮补充不可省略的收尾约束：

```text
在完成实现后：
1. 运行与本任务相关的测试或构建；报告命令与结果。
2. 检查改动范围。
3. 为本轮改动创建清晰的 git commit，并推送到指定分支。
4. 最终仅输出文字摘要：完成内容、改动文件、测试结果、commit SHA、推送结果、遗留风险。
5. 若不能安全完成任一步骤，停止，不要自行扩展任务；说明阻塞原因。
```

中间件保存 App Server 全量事件用于诊断，但正常向 ChatGPT 回传 Codex 的最终文字摘要、分支与 commit SHA。

## 8. 桌面界面

主界面默认简洁：

- 当前仓库、分支、模块名称。
- 当前阶段、轮次 `当前 / 上限`、已运行时间 `当前 / 上限`。
- Codex 最新状态和最近一次 commit SHA。
- “暂停”、“急停”、“打开 ChatGPT 标签页”和“查看诊断日志”操作。

暂停卡片提供：

- 验收通过：标记模块完成。
- 继续：清除当前暂停并继续等待/执行下一步骤。
- 终止：停止模块，保留记录。
- 重新规划：向 ChatGPT 发送用户输入并重新进入规划。

暂停或阻塞时发送 Windows 通知和声音。

## 9. 技术架构

- **桌面壳**：Tauri + React，持久化 SQLite，Windows 首发。
- **ChatGPT 适配器**：Manifest V3 Chrome 扩展 + 本机回环 WebSocket。扩展仅匹配 `https://chatgpt.com/*`，读取指定会话、等待流式回答完成、注入消息并返回 DOM 事件。
- **编排服务**：桌面进程中的状态机，串行队列、预算计时器、协议校验器、通知服务。
- **Codex 适配器**：子进程启动 `codex app-server`，经 stdio JSON-RPC 创建线程、开始 turn、监听事件与处理审批。
- **Git 适配器**：只读检查仓库、分支和推送状态；Codex 是提交与推送的执行者。中间件验证最终 SHA 已存在于远程分支。

App Server 是官方定义的自定义 Codex 客户端接口，适合获取会话、审批和流式 Agent 事件；将监听限定为本地 stdio，避免暴露网络端口。[官方文档](https://learn.chatgpt.com/docs/app-server)

## 10. 安全与可靠性

- Chrome 采用专用 Profile 和专用 ChatGPT 标签页；自动运行期间由中间件独占。
- 扩展不读取其他网站；本机连接使用一次性配对密钥。
- App Server 默认使用 stdio；不开放远程 WebSocket。
- 中间件不保存 ChatGPT 密码、Cookie 或 Git 凭证；使用浏览器和 Git 已登录会话。
- 推送前由中间件校验仓库路径、目标远程与选定分支；不同仓库或分支则暂停。
- 所有协议消息、App Server 事件、Git SHA、暂停原因写入本机审计日志。
- ChatGPT DOM 结构变化会导致扩展失效；必须具备“适配器不可用 → 暂停并通知”的降级路径。

## 11. MVP 验收标准

1. 用户可从桌面应用选择一个仓库、分支和已绑定的 ChatGPT 标签页后启动模块。
2. 系统可自动完成至少两轮 `ChatGPT → Codex → ChatGPT` 闭环，无人工复制粘贴。
3. 每轮代码改动均有测试结果、commit SHA 和成功推送记录。
4. 轮次与时间预算生效，并在当前轮结束后进入验收暂停。
5. Codex 或协议异常会在一分钟内暂停，并显示可操作的阻塞摘要与 Windows 通知。
6. 用户可在暂停卡片中完成四种操作，状态在重启应用后仍可恢复。

## 12. 实施顺序

1. 建立 Tauri 桌面壳、SQLite 状态存储、单模块状态机与模拟适配器。
2. 接入 App Server，完成单回合任务、事件流、最终摘要和失败处理。
3. 构建 Chrome 扩展与本机配对，完成协议发送、回复检测和 JSON 校验。
4. 接入 Git 推送验证、预算、暂停卡片、通知与日志。
5. 使用一个个人仓库进行两轮闭环的端到端验收；再处理 ChatGPT 页面结构变化和断线恢复。
