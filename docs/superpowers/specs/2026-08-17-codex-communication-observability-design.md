# Conversation Relay V2：Codex 通讯可观测性设计

- 日期：2026-08-17
- 状态：设计规格
- 范围：仅增加 Codex 通讯的结构化持久化、查询与桌面端可观测性；不改变 ChatGPT/Codex 协议或执行语义。

## 目标

让用户无需检查浏览器控制台、数据库或日志字符串，即可从桌面端连续观察一个 `CODEX_PROMPT` cycle 从接收、执行、排队回传到 ChatGPT 送达的状态。可观测性必须准确区分：Codex 已完成但正在等待 ChatGPT 通道，与 Codex 仍在运行。

本设计遵守 [004 — Conversation relay V2](../../decisions/004-conversation-relay-v2.md)：ChatGPT 出站消息全局严格 FIFO、同一时刻最多一个 ChatGPT 回复在途、未知送达不得自动重发、Codex 使用中间件持有的 App Server 与 thread。

## 非目标

第一版不增加以下能力：

- 强制取消 Codex turn；
- 强制释放 ChatGPT 通道；
- 多 Codex 并发；
- 原始 App Server event viewer；
- 编辑或重发 Codex prompt；
- 历史导出。

不修改 Codex App Server、ChatGPT terminal control-block 协议、浏览器 adapter 协议、消息队列的 FIFO 规则、模块生命周期或不确定送达规则。

## 用户界面

### 保持 ChatGPT 时间线纯净

现有「常驻 ChatGPT 对话」继续只展示实际传入和传出 ChatGPT 的消息。它不展示 `CODEX_TURN_STARTED`、App Server 状态、Codex final text 或其他 Codex 生命周期事件。Codex result 作为实际 `TO_CHATGPT` 消息，仍按既有规则出现在该时间线中。

### 顶部全局通道状态

工作区顶部新增 `GlobalChannelStatus`，由后端快照直接渲染，不从消息历史、事件文本或 CSS 状态推断。

| 通道 | 状态 | 必备字段 | 语义 |
| --- | --- | --- | --- |
| ChatGPT | `IDLE` | recovery blocker 数量 | 没有 `SENT` 或 `UNKNOWN` 的 `TO_CHATGPT` 消息。 |
| ChatGPT | `IN_FLIGHT` | 当前模块、当前 message ID、kind、当前阶段 | 存在唯一 `SENT` 的 `TO_CHATGPT` 消息；该消息已从 FIFO 取出，正在等待对应的 adapter/ChatGPT 完成结果。 |
| ChatGPT | `RECOVERY_BLOCKED` | `UNKNOWN` blocker 数量、全部 blocker 的摘要 | 至少一条 `UNKNOWN`；全局 FIFO 安全暂停，绝不自动重发。 |
| Codex | `IDLE` | 无当前 cycle/thread/turn | 没有正在运行的 Codex turn。 |
| Codex | `RUNNING` | 当前模块、cycle number、thread ID、turn ID（可得时）、当前状态 | 一个已启动但尚未结束的 Codex turn 正在运行。 |

ChatGPT 的「当前阶段」是队列/adapter 阶段而非控制块解析状态；Codex 的「当前状态」是本设计定义的 cycle 状态。若不存在 turn ID，快照返回 `null`，界面显示「尚未获得」而不是构造 ID。

### Codex 通讯面板

在 ChatGPT 时间线之外新增 `CodexCommunicationPanel`。它显示当前选中模块的 cycle，倒序排列，每个 `CodexCycleCard` 至少包含：

- cycle number；
- 当前用户状态；
- prompt 原文；
- Codex thread ID；
- Codex turn ID（尚不可得时明确显示「尚未获得」）；
- Codex final text（收到后立即显示）；
- outbound ChatGPT message ID（创建后显示）；
- 未继续时的明确阻塞原因。

当 cycle 已有 Codex final text、但另一个模块的 ChatGPT 消息占用全局通道时，卡片状态为「等待回传 ChatGPT」，阻塞原因必须写明占用模块名和当前 message ID。它不得显示为「Codex 运行中」。

当全局存在 `UNKNOWN` 时，面板显示「存在待人工处理的不确定送达消息」，并列出阻塞摘要；对应 cycle 的阻塞原因引用该状态。恢复入口沿用既有全局恢复 UI，提供「明确重发这条消息」和「不重发并继续」。Codex 面板不提供第二套重发入口。

## Cycle 状态机

`relay_codex_cycles.status` 是当前状态，且第一版只使用以下固定值和中文显示：

| 存储值 | 中文状态 | 进入条件 | 离开条件 |
| --- | --- | --- | --- |
| `WAITING_TO_SEND_CODEX` | 等待发送 Codex | 已验证 `CODEX_PROMPT` 并持久化 cycle，尚未提交 turn。 | 成功提交 turn、或启动失败。 |
| `CODEX_RUNNING` | Codex 运行中 | 已获得/提交 Codex turn。 | 收到 final text、或 turn 失败。 |
| `CODEX_COMPLETED` | Codex 已完成 | final text 已持久化；此状态和 `codex_completed_at` 必须先写入。 | 结果 `TO_CHATGPT` 消息已创建。 |
| `WAITING_FOR_CHATGPT` | 等待回传 ChatGPT | result 消息已入全局 FIFO，但尚未成为在途消息；也用于被 `UNKNOWN` blocker 阻塞。 | result 消息开始发送、或用户对自身 `UNKNOWN` 选择不重发。 |
| `SENDING_TO_CHATGPT` | 回传 ChatGPT 中 | 对应 result 消息变为 `SENT`。 | 收到对应 `chatgptReply`、adapter/transport 失败、或重启把在途消息标为 `UNKNOWN`。 |
| `DELIVERED_TO_CHATGPT` | 回传完成 | 中间件接受与该 outbound message ID 匹配的完成 `chatgptReply`。 | 终态。 |
| `FAILED` | 失败 | Codex turn 失败、result 发送失败后用户明确不重发、或其他不可继续的 cycle 层失败。 | 终态。 |

`CODEX_COMPLETED` 可能迅速转为「等待回传 ChatGPT」，因此卡片除当前状态外必须保留并展示已完成步骤及 `codex_completed_at`，保证用户可直接看到「Codex 已完成：<final text>」。

状态转换不重跑 Codex。对同一 cycle，Codex final text 一旦持久化，后续恢复仅操作已存在的 outbound ChatGPT message：

- 用户明确重发时，将同一 message ID 重新进入 FIFO；不创建第二条 result 消息；
- 用户明确不重发并继续时，该 message 保留为 `FAILED`，cycle 转为 `FAILED` 并记录用户决定；不发送旧消息；
- 应用重启把 `SENT` 转为 `UNKNOWN` 时，不自动重发，也不重新运行 Codex。

## 结构化持久化

新增 `relay_codex_cycles`。该表是 Codex 通讯面板和 channel snapshot 的唯一 cycle 数据源，日志和 `relay_events` 仅作审计，不参与 UI 状态推断。

| 字段 | 类型/约束 | 说明 |
| --- | --- | --- |
| `id` | UUID，主键 | cycle 标识。 |
| `module_id` | 外键/索引 | 所属 relay module。 |
| `cycle_number` | 整数；`(module_id, cycle_number)` 唯一 | 模块内从 1 递增；一个有效 `CODEX_PROMPT` 只占用一个编号。 |
| `status` | 非空枚举 | 上表七个固定值之一。 |
| `prompt_text` | 非空文本 | 原样保存的 `CODEX_PROMPT` body。 |
| `codex_thread_id` | 可空文本 | 中间件持有的 thread。 |
| `codex_turn_id` | 可空文本 | App Server 可获得时写入。 |
| `result_text` | 可空文本 | 只保存一次的 Codex final text。 |
| `outbound_chatgpt_message_id` | 可空；唯一 | `relay_messages.id`，只指向该 cycle 的唯一 result 消息。 |
| `error_text` | 可空文本 | 可行动的失败或阻塞说明；不保存凭据。 |
| `created_at` | 非空时间 | 已接收 `CODEX_PROMPT` 的时间。 |
| `codex_started_at` | 可空时间 | turn 已启动。 |
| `codex_completed_at` | 可空时间 | final text 已持久化。 |
| `relay_queued_at` | 可空时间 | result 消息已创建并入 FIFO。 |
| `relay_delivered_at` | 可空时间 | 匹配的完成 `chatgptReply` 已被接受。 |
| `updated_at` | 非空时间 | 最近一次结构化状态变化。 |

数据库写入使用事务，使 cycle 状态与关联 `relay_messages` 状态不会出现 UI 可见的半完成组合。`result_text` 和 `outbound_chatgpt_message_id` 一经写入不可被第二次生成覆盖；重复的 App Server completion 或重复 adapter reply 只能命中同一记录并被去重。

## 结构化生命周期记录

每次下列转换既更新 `relay_codex_cycles`，也追加同名 `relay_events` 审计事件。事件 detail 仅包含 IDs、状态和面向用户的错误摘要，不包含 pairing secret、Cookie、密码或原始 App Server 凭据。

| 事件 | 原子更新 |
| --- | --- |
| `CODEX_PROMPT_RECEIVED` | 创建唯一 cycle，状态为 `WAITING_TO_SEND_CODEX`。 |
| `CODEX_TURN_STARTED` | 写入 thread/turn ID（可得部分）和 `codex_started_at`，状态为 `CODEX_RUNNING`。 |
| `CODEX_RESULT_RECEIVED` | 一次性写入 `result_text`、`codex_completed_at`，状态为 `CODEX_COMPLETED`。 |
| `CODEX_RESULT_QUEUED_TO_CHATGPT` | 创建唯一 `TO_CHATGPT` result 消息，关联其 ID，写入 `relay_queued_at`，状态为 `WAITING_FOR_CHATGPT`。 |
| `CODEX_RESULT_SEND_STARTED` | result 消息成为全局在途 `SENT`，状态为 `SENDING_TO_CHATGPT`。 |
| `CODEX_RESULT_DELIVERED_TO_CHATGPT` | 接受匹配的完成 `chatgptReply`，写入 `relay_delivered_at`，状态为 `DELIVERED_TO_CHATGPT`。 |
| `CODEX_TURN_FAILED` | 写入 `error_text`，状态为 `FAILED`；不伪造 result 消息。 |

若 result 消息因 adapter/transport/restart 变为 `UNKNOWN`，不新增 Codex event，不改变 `result_text`，并将关联 cycle 置为 `WAITING_FOR_CHATGPT`，在 `error_text` 写入「存在待人工处理的不确定送达消息」。这表示等待用户处理 ChatGPT 送达，而不是 Codex 仍运行。

## 后端查询契约

新增以下 Tauri commands：

### `list_relay_codex_cycles(module_id)`

返回指定模块的 `relay_codex_cycles`，按 `cycle_number DESC` 排列。每项返回表中全部 UI 字段，以及计算好的 `blockReason`。`blockReason` 的来源只能是结构化 cycle 状态、关联 message 状态和 channel snapshot；前端不解析事件 detail。

### `get_relay_channel_snapshot()`

返回一次读取事务内构造的对象：

```text
{
  chatgpt: {
    status: "IDLE" | "IN_FLIGHT" | "RECOVERY_BLOCKED",
    activeModuleId: string | null,
    activeModuleName: string | null,
    activeMessageId: string | null,
    activeKind: "MANUAL" | "AUTOMATION" | null,
    activePhase: string | null,
    recoveryBlockerCount: number
  },
  codex: {
    status: "IDLE" | "RUNNING",
    activeModuleId: string | null,
    activeModuleName: string | null,
    cycleNumber: number | null,
    codexThreadId: string | null,
    codexTurnId: string | null,
    cycleStatus: string | null
  }
}
```

优先级固定如下，避免矛盾快照：ChatGPT 先判断 `UNKNOWN`（`RECOVERY_BLOCKED`），再判断唯一 `SENT`（`IN_FLIGHT`），否则 `IDLE`；Codex 有 `CODEX_RUNNING` cycle 时为 `RUNNING`，否则 `IDLE`。持久化约束保证最多一个 `CODEX_RUNNING` cycle。

桌面端在初始加载、模块切换、`relay-control`、`chatgpt-status` 和 Codex lifecycle 通知后刷新这两个结构化查询。`GlobalChannelStatus`、`CodexCommunicationPanel`、`CodexCycleCard` 是推荐的前端拆分边界。

## 正常与异常链路

```text
ChatGPT CODEX_PROMPT
  -> 创建 cycle（等待发送 Codex）
  -> 启动 Codex（Codex 运行中）
  -> 持久化 final text（Codex 已完成）
  -> 创建唯一 TO_CHATGPT result（等待回传 ChatGPT）
  -> 全局 FIFO 选中该消息（回传 ChatGPT 中）
  -> 接受匹配 chatgptReply（回传完成）
```

若另一模块占用 ChatGPT 通道，final text 和唯一 outbound message 仍立即持久化；cycle 等待该通道，不重新运行或重复排队 Codex。若存在任意 `UNKNOWN`，快照为 `RECOVERY_BLOCKED`，所有 FIFO 派发暂停，直到用户逐条明确解决。恢复后只继续既有队列；不会自动发送、重跑 Codex 或生成第二条 result。

## 验证计划

后端自动化测试覆盖：

1. 一个有效 `CODEX_PROMPT` 只创建一个 cycle；
2. Codex running 与 completed 状态、thread/turn/result 均结构化持久化；
3. final text 仅排队一条 outbound ChatGPT result；
4. Codex 已完成但另一个模块占用 ChatGPT 通道时，cycle 为「等待回传 ChatGPT」并带占用模块原因；
5. `UNKNOWN` 使 channel snapshot 为 `RECOVERY_BLOCKED` 且显示 blocker 数量；
6. result 的 `SENT -> DELIVERED` 显示「回传 ChatGPT 中 -> 回传完成」；
7. restart 的 `SENT -> UNKNOWN` 不会自动重发或重新运行 Codex；
8. Codex failure 不创建或伪造 result。

React 自动化测试覆盖：

1. `CodexCycleCard` 显示 prompt、thread、turn、result 与 block reason；
2. 全局通道状态使用 snapshot 显示 recovery blocked；
3. ChatGPT 时间线不混入 Codex lifecycle events；
4. 等待其他模块释放 ChatGPT 通道不会显示为 Codex 运行中。

E2E 验收在 UI 中依次可见：收到 `CODEX_PROMPT`、Codex 运行中、Codex 已完成且展示 `RELAY_E2E_OK`、等待或开始回传 ChatGPT、回传完成。该验收不要求、也不引入自动重发或强制释放通道。
