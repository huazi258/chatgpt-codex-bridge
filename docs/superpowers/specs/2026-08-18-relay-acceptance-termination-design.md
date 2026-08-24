# Conversation Relay V2：模块验收与终止设计

- 日期：2026-08-18
- 状态：设计规格
- 范围：定义 relay module 的人工验收、验收反馈、用户终止、运行时释放和迟到事件处理；不实现本设计。

## 目标与边界

本设计为 [004 — Conversation relay V2](../../decisions/004-conversation-relay-v2.md) 的 `MODULE_DONE`、人工验收和终止语义提供可实施的产品状态机。用户是唯一能够验收通过或终止模块的主体；ChatGPT 的 `@@@MODULE_DONE@@@` 只请求人工验收，绝不自行完成模块。

本设计保持以下既有边界：

- 不改变 `CODEX_PROMPT`、`MODULE_DONE` 或 `BLOCKED` 控制块语法；V2 不支持 Codex App Server 人工输入交互，也不恢复 ChatGPT `CODEX_INPUT` 控制块；
- 所有 ChatGPT 出站消息继续进入现有全局严格 FIFO，且最多一个回复在途；
- `UNKNOWN` 不自动重发，且只能由用户现有的明确恢复动作解决；
- 仅允许一个 middleware-owned Codex runtime/active turn；
- 不删除 Codex thread，不实现 released-thread resume，不强杀运行中的 Codex turn；
- 不改变已确认的 [Codex 通讯可观测性设计](2026-08-17-codex-communication-observability-design.md) 的七个 cycle 状态或其送达关联语义。

## 当前实现基线与既有缺口

当前 HEAD `819a98d` 已有 `relay_modules.phase`、`stop_after_turn`、`relay_codex_cycles`、全局 FIFO 和 `WAITING_FOR_ACCEPTANCE` 的控制块识别，但尚未具备本设计的产品动作。

| 当前项目 | 已有行为 | 本设计需要补齐的缺口 |
| --- | --- | --- |
| `MODULE_DONE` | `handle_relay_chatgpt_reply` 写入 `WAITING_FOR_ACCEPTANCE` 和事件。 | 没有 accept、反馈或终止命令及 UI。 |
| `stop_after_turn` | 已持久化并暴露给模块记录，但没有任何读取或写入的运行时逻辑。 | 需要作为运行中终止请求的唯一持久化意图。 |
| Codex worker | `RelayCodexSession` 只有 `StartTurn` sender；worker 在循环退出后 `child.kill()`/`wait()`，没有主动 release 命令或 join/ack。 | 需要最小 `Release` runtime 命令与确认，避免终态模块继续占用唯一 Codex runtime。 |
| Codex completion | final text 保存为 `FROM_CODEX`，并总是创建/排队一条 result `TO_CHATGPT`。 | 终止请求后的最终结果必须保存，但不得创建或派发 result。 |
| ChatGPT reply | 匹配 `SENT` 后总会保存回复并对自动化回复解析控制块。 | `STOPPED`/`COMPLETED` 的迟到回复仍需保存，但不得解析或恢复自动化。 |
| FIFO claim | 选择所有 `TO_CHATGPT/QUEUED`。 | 必须防御性排除终态模块，终态模块遗留的 queued 记录不得被 claim。 |
| 终态发送 | `queue_relay_message` 已拒绝 `STOPPED`/`COMPLETED`。 | 还需在终态转换中明确处理既有 queued、sent 和 unknown 消息。 |

这些缺口均为实现工作，不构成新的产品决策；本设计确定其语义。

## 模块 phase 与终态

本设计沿用当前 relay phase：`READY`、`CODEX_PROMPT_READY`、`CODEX_STARTING`、`CODEX_RUNNING`、`WAITING_FOR_ACCEPTANCE`、`BLOCKED`、`RECOVERY_REQUIRED`、`COMPLETED`、`STOPPED`。实现不得把 `COMPLETED` 或 `STOPPED` 恢复成任何非终态 phase。

| phase | 含义 | 可执行的产品动作 |
| --- | --- | --- |
| `WAITING_FOR_ACCEPTANCE` | ChatGPT 已请求结束，自动化暂停，等待用户检查代码、测试和结果。 | 接受并完成、提交验收反馈并继续、终止模块。 |
| 其他非终态 | 模块仍可运行、等待 ChatGPT、阻塞或等待恢复。 | 终止模块；普通消息/自动化仍受各自既有规则约束。 |
| `COMPLETED` | 用户验收通过的终态。 | 无发送、无终止、无新 Codex turn。 |
| `STOPPED` | 用户主动终止的终态，不表示验收通过。 | 无发送、无终止、无新 Codex turn。 |

`stop_after_turn = 1` 不是新的 phase。它只表示用户已请求终止且当前 Codex turn 必须自然结束；此时模块保持 `CODEX_RUNNING`，UI 显示「终止已请求，等待当前 Codex 回合结束」。turn 收尾为 `STOPPED` 时该标志复位为 `0`，终止请求与释放结果由审计事件保留。

## 后端原子产品动作

前端不得用多个低层 command 拼装状态转换。新增以下等价 Tauri commands；所有前置检查、消息状态更新和事件追加均在同一数据库锁与事务中完成。前端 disabled 只用于体验，后端是唯一权威。

### `accept_relay_module(module_id)`

| 项目 | 规则 |
| --- | --- |
| 前置条件 | 模块存在且 `phase = WAITING_FOR_ACCEPTANCE`；`stop_after_turn = 0`；当前模块没有任何 `TO_CHATGPT/UNKNOWN`；没有该模块正在运行的 `CODEX_RUNNING` cycle/active worker turn。 |
| 拒绝 | 其他 phase、当前模块存在 `UNKNOWN`、或 Codex 意外仍在运行时返回明确中文错误，不改变数据。其他模块的 `UNKNOWN` 不阻止本动作。 |
| 事务后置条件 | 模块设为 `COMPLETED`；本模块所有 `TO_CHATGPT/QUEUED` 设为 `FAILED`，保留原文，原因为「模块已验收完成，消息未发送。」；追加 `MODULE_ACCEPTED` 与每条消息的未发送审计。不得新增 ChatGPT 消息或 Codex cycle。 |
| runtime 后置条件 | 立即请求 release 当前 middleware-owned Codex runtime；收到 release acknowledgement 后追加 `CODEX_THREAD_RELEASED`。`relay_modules.codex_thread_id` 永远保留。 |
| 幂等 | 已完成模块重复调用返回「模块已验收完成」的无副作用成功；其他终态或非等待验收状态明确拒绝。不会重复释放或追加验收事件。 |

接受是用户验收，不是 ChatGPT 的完成声明。`COMPLETED` 后不发送额外自动化消息，不再启动新 turn，全部消息、cycle、event 和审计历史继续可读。

### `submit_relay_acceptance_feedback(module_id, text)`

| 项目 | 规则 |
| --- | --- |
| 前置条件 | 模块存在、`phase = WAITING_FOR_ACCEPTANCE`，且 `text.trim()` 非空。 |
| 事务后置条件 | 写入一条 `TO_CHATGPT/AUTOMATION/QUEUED`，使用既有 `queue_relay_message` 的 sequence、事件和全局 FIFO 语义；模块设为 `WAITING_FOR_CHATGPT`，表示等待该自动化消息的队列/回复结果。 |
| 发送 | 事务提交后调用现有全局 dispatcher；不得走手动聊天、直接 WebSocket 或旁路队列。若其他模块 `UNKNOWN` 阻塞，消息保持可见 `QUEUED`，全局 recovery UI 继续说明原因。 |
| 后续 | 匹配回复属于自动化回复，继续使用既有 terminal control-block parser；新的有效 `CODEX_PROMPT` 在同一模块使用保留的 middleware-owned thread，并正常增加 `started_cycles`。 |
| 幂等 | 因为首次提交原子地离开 `WAITING_FOR_ACCEPTANCE`，重复提交被明确拒绝；不会生成第二条反馈消息。 |

反馈提交不清除本模块或其他模块的 `UNKNOWN`。它不属于 accept/terminate 的结束动作，因此存在本模块 `UNKNOWN` 时仍可入队，但必须显示既有 recovery blocker，且不会被自动发送。

### `terminate_relay_module(module_id)`

| 项目 | 无运行 turn | 有运行 turn |
| --- | --- | --- |
| 前置条件 | 模块为非终态，当前模块无 `UNKNOWN`。 | 同左。 |
| 立即 DB 行为 | 模块转为 `STOPPED`；本模块 `TO_CHATGPT/QUEUED → FAILED`，原因为「模块已由用户终止，消息未发送。」；追加 `MODULE_TERMINATED`。 | `stop_after_turn = 1`；保留 `CODEX_RUNNING`；本模块既有 `TO_CHATGPT/QUEUED → FAILED` 并记录终止请求。 |
| 立即 runtime 行为 | 请求并等待 runtime release acknowledgement。 | 不发送 release、不强杀 turn；UI 显示等待当前回合。 |
| 收尾 | release 成功后追加 `CODEX_THREAD_RELEASED`，保留 thread ID。 | 当前 turn 自然结束后保存 final text；不创建 result outbound message；模块转 `STOPPED`，复位 `stop_after_turn`，再 release。 |
| 重复调用 | `STOPPED` 返回无副作用「模块已终止」；`COMPLETED` 拒绝；stop request 已存在时返回无副作用「终止已请求」。 | 同左。 |

`UNKNOWN` 属于当前模块时，accept 与 terminate 均必须拒绝并提示「请先处理本模块的不确定送达消息」。它绝不因结束模块而被改写为 `FAILED`。其他模块的 `UNKNOWN` 只阻塞全局 ChatGPT 派发，不阻止当前模块自身无 `UNKNOWN` 的 accept、terminate 或 runtime release。

## 消息在结束模块时的处理

| 结束时 delivery state | accept | terminate | 之后的规则 |
| --- | --- | --- | --- |
| `QUEUED` | `FAILED`，保留正文和「模块已验收完成，消息未发送。」审计。 | `FAILED`，保留正文和「模块已由用户终止，消息未发送。」审计。 | 全局 FIFO 永不再 claim。 |
| `SENT` | 不伪造取消，不改为 `FAILED`。 | 不伪造取消，不改为 `FAILED`。 | 若收到匹配 reply，正常完成 delivery 并保存真实 `FROM_CHATGPT`；不启动自动化。 |
| `UNKNOWN` | 拒绝 accept。 | 拒绝 terminate。 | 只能使用既有「明确重发」或「不重发并继续」。 |
| `DELIVERED` / 既有 history | 不改写。 | 不改写。 | 保留完整历史。 |

dispatcher 的候选查询必须 join `relay_modules` 并排除 `phase IN ('COMPLETED', 'STOPPED')`。这是终态转换批量更新之外的第二道防线：即使数据库中存在遗留 `QUEUED`，也不会被 global FIFO claim。实现发现这种遗留记录时必须留下审计，不得静默发送或删除。

## 运行中 Codex turn 的终止时序

```text
用户确认终止
  -> 原子检查：模块非终态、当前模块无 UNKNOWN、cycle/worker 正在运行
  -> stop_after_turn = 1，既有 QUEUED 均 FAILED，记录 MODULE_STOP_REQUESTED
  -> UI：终止已请求，等待当前 Codex 回合结束
  -> 当前 turn 自然结束（不强杀）
  -> 持久化 result_text、codex_completed_at、FROM_CODEX 历史
  -> cycle 保持 CODEX_COMPLETED；不创建 outbound_chatgpt_message_id
  -> cycle block reason：模块已由用户终止，结果未回传 ChatGPT
  -> relay_modules.phase = STOPPED，stop_after_turn = 0
  -> Release runtime，worker 退出，CODEX_THREAD_RELEASED
```

这条路径不把用户主动终止误报为 Codex 失败。`CODEX_COMPLETED` 是现有七态中唯一准确表达「Codex 已完成且 final text 已保存」的状态；因为没有 outbound message，它不转为 `WAITING_FOR_CHATGPT`。`list_relay_codex_cycles` 必须根据 module 的 `STOPPED` 和该 cycle 没有 outbound message 计算上述 block reason。

如果 turn 实际以 App Server error/non-completed status 结束，cycle 仍可为 `FAILED`，因为这是实际 Codex 执行失败；模块随后仍收尾为 `STOPPED`，不恢复自动化。

Codex App Server 人工输入交互不属于 V2 middleware 功能。终止语义不依赖该功能，也不定义替代的 agent-level 规则。

## thread release 最小接口

当前 worker 仅接受 `StartTurn`，无法由终态模块主动退出。实现新增最小内部 runtime 命令，不建立通用 App Server 管理框架：

```text
RelayCodexCommand::Release { acknowledgement }
```

规则如下：

1. 非运行 turn 的 accept/terminate 将 `Release` 发送给当前模块 session；worker 停止接收新的 turn，退出循环，关闭 stdin，终止/等待其子进程，然后发送 acknowledgement。
2. 运行 turn 的 terminate 不发送 `Release`；只有 `turn/completed` 或实际失败处理完持久化后才发送。
3. acknowledgement 后，只有匹配 module 的 `state.relay_codex` 被清为 `None`；不得清除另一个模块已创建的 session。
4. command 在 acknowledgement 前不报告「thread 已释放」。若 release 失败或超时，模块维持终态，记录可行动的 `CODEX_THREAD_RELEASE_FAILED`，且该 session 继续阻止新模块获得 Codex runtime，直至 worker 确认退出或应用关闭。
5. release 不删除数据库的 `codex_thread_id`、不删除 App Server thread，也不实现 `thread/resume`。

该顺序确保终态模块不长期把唯一 Codex runtime 伪装为可用；成功 release 后下一个模块可以获得新的 runtime。

## 异步事件与竞态规则

所有异步 worker、adapter 和 UI 回调在执行副作用前重新读取模块 phase/`stop_after_turn`；后端状态机优先于前端按钮状态。

| 竞态 | 规则 |
| --- | --- |
| 用户 accept 时 Codex 意外运行 | accept 原子拒绝，提示先等待当前 turn 或选择终止；不得完成模块。 |
| terminate 与 Codex final 同时发生 | 先获得数据库锁的一方决定结果。若 terminate 先设置 `stop_after_turn`，final 仅保存，不排队；若 final 已完成并已排队，terminate 仅将尚未 `SENT` 的 result 设为 `FAILED`，不创建替代消息。 |
| terminate 与 `chatgptReply` 同时发生 | `SENT` reply 的 delivery/history 可完成；终态 gate 阻止控制块解析、cycle 创建和 Codex 启动。 |
| duplicate terminate | 第一次写入意图；第二次根据 `stop_after_turn` 或 `STOPPED` 无副作用返回。 |
| duplicate accept | 已 `COMPLETED` 无副作用成功；从其他 phase 明确拒绝。 |
| duplicate feedback | 第一次原子离开 `WAITING_FOR_ACCEPTANCE`；第二次明确拒绝。 |
| terminal module 的旧 QUEUED 被 dispatcher 看见 | 查询过滤终态；终态动作也已把记录置 `FAILED`，因此绝不 claim。 |
| 应用重启 | 既有 `SENT → UNKNOWN` 规则先执行；终态模块不被恢复为活动模块，`UNKNOWN` 仍要求明确用户恢复。 |

`handle_relay_chatgpt_reply` 的顺序必须为：匹配 `SENT` request ID、完成送达关联、保存真实 `FROM_CHATGPT`，然后读取模块 phase。若 phase 为 `STOPPED` 或 `COMPLETED`，追加 `LATE_CHATGPT_REPLY_IGNORED`，跳过 terminal-control-block parser、invalid count 更新、retry、cycle 创建和 Codex 启动。该路径不得改变终态。

## 用户界面

### 验收卡

当模块为 `WAITING_FOR_ACCEPTANCE`，在模块状态区域显示独立卡：

```text
等待人工验收
ChatGPT 已请求结束本模块。
请检查代码、测试和结果。

[ 接受并完成模块 ]

验收反馈：
[ textarea ]
[ 提交反馈并继续 ]

[ 终止模块 ]
```

反馈为空时禁用提交并显示「请填写验收反馈」。接受与终止在当前模块存在 `UNKNOWN` 时禁用，并显示「请先处理本模块的不确定送达消息」。终止始终要求二次确认，确认文案区分运行中和非运行中：运行中明确说明不会强杀当前 Codex 回合且结果不会回传 ChatGPT。

### 全局终止入口与终态

所有非终态模块在模块状态区域显示「终止模块」；`WAITING_FOR_ACCEPTANCE` 使用同一个终止动作。`stop_after_turn = 1` 时按钮禁用并显示「终止已请求，等待当前 Codex 回合结束」。

| 模块状态 | UI |
| --- | --- |
| `COMPLETED` | 显示「已验收完成」；隐藏 composer、验收动作与终止按钮。 |
| `STOPPED` | 显示「已终止」；隐藏 composer、验收动作与终止按钮。 |
| 当前模块 `UNKNOWN` | 显示恢复提示与既有恢复入口；accept/terminate 不可执行。 |
| 其他模块 `UNKNOWN` | 继续显示全局 recovery blocker；不禁用当前模块的 accept/terminate。 |

后端拒绝必须以明确中文显示在现有 notice 中，例如「当前 Codex 回合仍在运行，不能验收完成」或「请先处理本模块的不确定送达消息」。

## 测试矩阵

Rust 自动化测试至少覆盖：

1. `MODULE_DONE → WAITING_FOR_ACCEPTANCE`；
2. 只有 `WAITING_FOR_ACCEPTANCE` 可 accept；
3. accept 后为 `COMPLETED`，保留 `codex_thread_id` 并请求一次 release；
4. `COMPLETED` 拒绝新消息；
5. 验收反馈作为唯一 `AUTOMATION/TO_CHATGPT` 进入既有 FIFO，反馈后有效 `CODEX_PROMPT` 复用 thread 且增加 cycle；
6. 非运行模块 terminate 后为 `STOPPED`，release，保留历史和 thread ID；
7. 运行 Codex terminate 只设 `stop_after_turn`，不强杀；
8. 运行 turn final text 被保存、cycle 为 `CODEX_COMPLETED`、没有 outbound result，随后 `STOPPED` 与 release；
9. terminate 将本模块 `QUEUED` 置 `FAILED` 并追加审计；
10. 当前模块 `UNKNOWN` 拒绝 accept/terminate，其他模块 `UNKNOWN` 不阻止本模块结束；
11. `STOPPED`/`COMPLETED` 的匹配 `SENT` reply 保存 delivery 和 `FROM_CHATGPT`，但不解析控制块、不启动 Codex、不改变 phase；
12. duplicate accept/terminate/feedback 的幂等或明确拒绝语义；
13. terminate 与 completion race 不产生第二条或新的 outbound result；
14. terminal module 的 queued 记录永不被全局 FIFO claim；
15. restart 后终态不恢复活动，`SENT → UNKNOWN` 仍不自动重发。

React 自动化测试至少覆盖：

1. `WAITING_FOR_ACCEPTANCE` 显示接受、反馈、终止三个动作；
2. 空反馈不可提交；
3. 终止要求二次确认；
4. `stop_after_turn` 显示等待当前回合且阻止重复点击；
5. `COMPLETED` 显示「已验收完成」并隐藏 composer；
6. `STOPPED` 显示「已终止」并隐藏 composer；
7. 当前模块 `UNKNOWN` 显示先恢复提示并禁用 accept/terminate；
8. 后端拒绝显示明确中文错误；
9. 终止后 completed Codex cycle 显示「模块已由用户终止，结果未回传 ChatGPT」，且 ChatGPT 时间线不新增伪造 lifecycle 消息。

本设计不扩展到 Codex 人工输入流程、existing Codex thread resume、browser-history sync、多 Codex 并发、强杀 turn、删除 Codex thread、新 ChatGPT 控制块、新 browser adapter 协议或任意队列管理 UI。
