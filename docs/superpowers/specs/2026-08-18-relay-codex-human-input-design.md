# Conversation Relay V2：Codex 人工输入请求设计

- 日期：2026-08-18
- 状态：设计规格
- 范围：定义 Codex App Server user-input request 的持久化、用户交互、原请求响应、终止与恢复语义；不实现本设计。

## 目标与边界

Codex 在一个已启动的 turn 中请求用户输入时，middleware 直接向用户展示问题并收集答案，再把答案响应到同一个 App Server request。该路径是 Codex 与用户之间的本地人工干预，不经过 ChatGPT。

本设计取代 ChatGPT `@@@CODEX_INPUT@@@` 控制块。`CODEX_INPUT` 与 `@@@END_CODEX_INPUT@@@` 不再是 V2 控制协议的一部分，也不得由 ChatGPT automation reply 触发或响应 Codex input。

以下边界固定：

- 一次 input request 不创建新的 relay cycle、Codex turn 或 Codex thread；它只继续原有运行中的 turn。
- 用户逐题填写自由文本；App Server 给出的 options 仅作参考展示，不能限制、校验或替换用户答案。
- `AnswerInput` 只能响应已持久化的原 App Server request，绝不能调用 `turn/start`。
- UI 按 App Server 原始顺序展示问题，但 wire-level 问题身份只能是 `question.id`；显示文本、header 和数组下标都不得用于构造回答。
- `isSecret = true` 的答案仅在内存中短暂存在以响应原 request，绝不写入 SQLite、relay event、日志或 diagnostic。
- ChatGPT FIFO、`UNKNOWN` recovery、automation terminal-control parser 和 automation retry 不参与本流程。
- 不增加多 Codex 并发、不强杀 turn、不编辑或重发 Codex prompt，也不新增 ChatGPT 控制块。

## 术语

| 术语 | 含义 |
| --- | --- |
| input request | Codex App Server 在既有 turn 内发出的、要求用户提供一个或多个答案的 request。 |
| pending request | 状态为 `PENDING`，仍可由用户回答的 input request。每个活跃 Codex turn 最多一个。 |
| original request | 由 App Server 赋予的 request ID、所属 thread/turn 和完整问题元数据；回答必须回到这一 request，并以每个 `question.id` 为 wire-level 身份，不能构造新 request。 |
| input session | middleware 对一个 `PENDING` request 的本地持久化与 UI 展示，不是新的 Codex thread 或 relay cycle。 |

## 用户流程

```text
Codex App Server user-input request
  -> middleware 验证其属于当前 active worker / thread / turn
  -> 原子持久化 relay_codex_input_requests = PENDING
  -> module 保持原运行 phase，UI 显示“等待你的 Codex 输入”
  -> 用户逐题输入自由文本并提交
  -> middleware 原子 claim 该 PENDING request
  -> worker 将 JSON-RPC response 写给原 App Server request，request 仍为 ANSWERING
  -> 匹配 app_server_request_id 的 serverRequest/resolved：request = ANSWERED，turn 继续运行
  -> 提交前先收到 serverRequest/resolved：request = EXPIRED，禁止迟到提交
```

提交通知只在本地 UI 与 worker 间流转：不写 `relay_messages`，不生成 `TO_CHATGPT` 或 `FROM_CHATGPT`，不增加 `invalid_reply_count`，也不消费 ChatGPT protocol retry。

## 持久化模型

新增独立实体 `relay_codex_input_requests`。它不替代 `relay_codex_cycles`；一个 cycle 可以有零个或多个依次完成的 input request，但同一 active turn 同时最多一个 `PENDING` request。

最小字段：

| 字段 | 规则 |
| --- | --- |
| `id` | middleware UUID，主键。 |
| `module_id` | 所属 relay module，外键。 |
| `cycle_id` | 所属已存在 `relay_codex_cycles` 记录，外键。 |
| `codex_thread_id` | 收到 request 时的 middleware-owned thread ID。 |
| `codex_turn_id` | 收到 request 时的 active turn ID。 |
| `app_server_request_id` | 原 App Server request ID；对仍可响应的 request 唯一。 |
| `questions_json` | 按 App Server 原始顺序完整持久化当前输入协议中与回答相关的原始问题元数据：至少 `id`、`header`、`question`、`options`、`isOther`、`isSecret`、`autoResolutionMs`（如有）以及实现响应当前协议所需的兼容字段。 |
| `answers_json` | 仅保存非 secret 问题的 `question.id → answers[]` 映射；`PENDING` 时为空。空回答为 `[]`，而不是 `[""]`。 |
| `secret_answer_status_json` | 仅保存 secret 问题的脱敏状态（例如 `question.id` 与 `provided: true/false`）；不得保存答案原值。 |
| `status` | `PENDING`、`ANSWERING`、`ANSWERED`、`INTERRUPTED` 或 `EXPIRED`。 |
| `error_text` | 非敏感、可行动的失败或过期原因；无错误时为空。 |
| `created_at` | 收到 App Server request 的时间。 |
| `answered_at` | 收到匹配 `serverRequest/resolved`、最终确认该 request 已回答的时间；否则为空。 |
| `interrupted_at` | 重启或答案送达结果不确定时标记中断的时间；否则为空。 |
| `expired_at` | App Server 已 resolved/cleared 或迟到提交被拒绝时的时间；否则为空。 |
| `updated_at` | 最后一次状态转换时间。 |

问题元数据与非 secret 答案使用 JSON 仅保存结构化的本地输入，不作为 ChatGPT 协议载荷。保存时不得记录 cookie、pairing secret、密码或其他凭据；secret 答案原值只可在用户提交到 worker 的短暂内存中存在，提交后立即丢弃。

### 问题身份、wire response 与答案值

UI 保持 `questions_json` 中的原始顺序：每题一个普通自由文本输入框，`header` 和 `question` 用于展示，`options` 仅作为参考，`isOther` 不增加强制表单动作。该交互不改变 App Server 的 wire identity。

middleware 构造 JSON-RPC response 时必须按原始 `question.id` 映射每题的 answers 列表；显示文本、header、options 和数组下标都不得作为 key。其 wire-level 语义为：

```text
question.id -> answers[]
```

每个已填写的普通自由文本框产生该题的单元素 `answers` 列表；用户留空则该题产生空列表 `[]`，绝不发送字符串数组 `[""]`。具体 JSON-RPC 外层 envelope 与兼容字段必须遵循收到的当前 App Server request 协议，但不得丢失 `questions_json` 保存的原始兼容元数据。

`isSecret = true` 时，可以展示问题、header 与参考 options，但输入值只交给内存中的 response 构造器。SQLite 中的 `answers_json` 不得有该原值，`secret_answer_status_json` 只可记录「已提供/未提供」，relay events、日志、diagnostic 与错误文本也不得包含原值。提交失败、进程中断或重启后，middleware 因不保留原值而不得自动恢复或重发 secret 答案。

若 request 提供 `autoResolutionMs`，middleware 将其作为兼容元数据持久化并可在 UI 中展示；本地倒计时不构成 request 已失效的事实。request 是否仍可回答只由 active runtime 状态与匹配的 App Server `serverRequest/resolved` 事件确定。

### 状态机

| 当前状态 | 触发 | 下一状态 | 规则 |
| --- | --- | --- |
| 无记录 | 收到匹配 active turn 的 App Server request | `PENDING` | 原子写入问题、原 request ID 与审计事件；不创建 cycle/turn/thread。 |
| `PENDING` | 用户点击提交 | `ANSWERING` | 同一事务 claim；按每个 `question.id` 构造 answers 列表，允许任意问题留空；只持久化非 secret 值和 secret 脱敏状态。 |
| `ANSWERING` | worker 已将 JSON-RPC response 写给原 request | `ANSWERING` | 只记录「响应已发出」；stdin 写成功或 worker acknowledgement 不能证明 App Server 已接受答案。 |
| `ANSWERING` | 收到匹配 `app_server_request_id` 的 `serverRequest/resolved` | `ANSWERED` | 这是唯一最终确认点；记录 `answered_at`，turn 继续。 |
| `ANSWERING` | worker 或本地 App Server transport 无法确认答案是否被接受 | `INTERRUPTED` | module 进入 `RECOVERY_REQUIRED`；不得自动重发答案或假定 request 仍可写。 |
| `PENDING` / `ANSWERING` | App/runtime 重启 | `INTERRUPTED` | 模块进入 `RECOVERY_REQUIRED`；不得自动恢复、重放或再次回答旧 request。 |
| `PENDING` | 收到匹配 `app_server_request_id` 的 `serverRequest/resolved`，但用户尚未提交 | `EXPIRED` | App Server 已先清除 request；禁止迟到提交。 |
| `PENDING` / `ANSWERING` | App Server 明确报告 request ID 不匹配、unknown 或已由其他路径 cleared，且没有本地已发送 response 的匹配 resolved 确认 | `EXPIRED` | 保存原因；不得发送或重发答案。 |

`ANSWERED`、`INTERRUPTED` 与 `EXPIRED` 均为终态。重复提交 `ANSWERED` 返回无副作用结果；对 `INTERRUPTED` 或 `EXPIRED` 返回明确中文错误并且绝不发送任何答案。

## Worker 与后端接口

`RelayCodexCommand` 增加最小命令：

```text
AnswerInput {
  input_request_id,
  app_server_request_id,
  answers,
  acknowledgement
}
```

发送该命令前，后端必须在事务内验证：模块非终态；input request 是当前 module/cycle/thread/turn 的 `PENDING` 记录；其 `app_server_request_id` 与当前 worker request 相同；每个 wire-level answer key 均为已持久化 `question.id`；且该 request 尚未被 App Server cleared。worker 只把答案写回原 App Server request。其 acknowledgement 至多报告 JSON-RPC response 已写出、已过期或 transport failure；它不得报告 `ANSWERED`。

`AnswerInput` 不得调用 `turn/start`、不得创建 `RelayCodexSession`、不得改变 `started_cycles`、不得更新 `codex_thread_id`，也不得排队任何 ChatGPT 消息。worker 在收到 App Server input request 后继续持有同一 turn；写出答案后仍等待同一 turn 的匹配 `serverRequest/resolved`、后续事件和自然完成。

建议新增 Tauri commands：

- `list_relay_codex_input_requests(module_id)`：返回模块的输入 request 历史和当前状态；不返回任何 ChatGPT transport 状态。
- `submit_relay_codex_input(input_request_id, answers)`：原子 claim 并向匹配 worker 发送 `AnswerInput`；返回 answering、response-sent、answered、expired 或 interrupted 的结构化中文结果，其中 `answered` 只能由匹配 resolved 事件返回。

前端不得直接调用 App Server，也不得根据日志文本猜测 request 是否存在或已经 resolved。

## Runtime、终止与预算

module runtime 从首个 Codex turn 开始持续计时，用户等待 input 的时间同样计入 runtime budget。达到 runtime budget 时，不强杀正在等待 input 的 turn。

如果用户 terminate 或 runtime budget 到期时存在 `PENDING` / `ANSWERING` request：

1. 不启动新 turn，不创建新 cycle，不把 final result 回传 ChatGPT。
2. 当前原 request 仍可完成必要的人工回答；UI 明确显示「模块已请求终止；可完成当前 Codex 输入，回合结束后停止」。
3. `AnswerInput` 接受后仅继续这一现有 turn。turn 自然完成后，保存 final text，module 转 `STOPPED`，并按既有终止语义 release runtime；final result 不进入 ChatGPT FIFO。
4. 若 request 在完成前已由 App Server cleared，则标记 `EXPIRED`；turn 以实际结果收尾，仍不得恢复自动化。

普通 `PENDING` input 不是 ChatGPT `BLOCKED`，也不是 `RECOVERY_REQUIRED`。只有重启中断、App Server request 过期/无法对应、或 worker/transport 失败才显示相应的可行动错误。runtime budget 的终止意图与用户 terminate 的 `stop_after_turn` 语义保持一致：它允许完成当前必要 input 和同一 turn，但禁止向 ChatGPT 回传 final result。

## 重启与迟到事件

应用或 runtime 重启恢复时，所有 `PENDING` 与 `ANSWERING` 记录必须原子标记为 `INTERRUPTED`，记录中断时间与原因，所属 module 转为 `RECOVERY_REQUIRED`。middleware 不得尝试恢复旧 App Server request、自动重发旧答案或假定旧 turn 仍可写入。用户必须检查当前状态并开始明确的后续恢复路径；本设计不定义自动恢复或重放。

若用户尚未提交时，App Server 先对原 request 发出匹配的 `serverRequest/resolved`，记录改为 `EXPIRED`。若 response 已写出但在收到匹配 resolved 前进程崩溃或 transport 中断，记录改为 `INTERRUPTED`，module 进入 `RECOVERY_REQUIRED`；不得根据已保存的答案自动重发。App Server 明确报告 request ID 不匹配、unknown 或已由其他路径 cleared 时同样标记 `EXPIRED`。UI 必须移除提交动作，保留问题、允许保留的非 secret 答案和原因以便审计；迟到的 `submit_relay_codex_input` 永远不发送到任何 App Server request。

## UI 与可观测性

当存在 `PENDING` request，模块工作区显示独立的「Codex 需要你的输入」卡，而不是在常驻 ChatGPT 对话中创建消息：

```text
Codex 需要你的输入
问题 1：<原问题>
参考选项：<如有，仅供参考>
[ 自由文本输入框 ]

问题 2：<原问题>
参考选项：<如有，仅供参考>
[ 自由文本输入框 ]

[ 提交给 Codex ]
```

卡片显示所属 cycle number、thread ID、turn ID、request 状态和阻塞原因。`ANSWERING` 禁止重复提交并显示「答案已发送，正在等待 Codex 确认」；`INTERRUPTED` 显示「输入请求因应用或运行时中断，请检查模块恢复状态」；`EXPIRED` 显示 App Server 已不再接受该 request 的原因。options 不做单选、多选或必填约束；用户可确认后提交含未回答问题的 request。`isSecret` 输入框不得在 UI 重新渲染、通知或错误提示中回显原值。

全局 Codex 通道状态继续显示运行中的 module/cycle/thread/turn。ChatGPT 全局通道状态不因 input request 进入 busy、recovery blocked 或 retry；它只反映 ChatGPT FIFO 自身。审计事件至少结构化记录：

- `CODEX_INPUT_REQUEST_RECEIVED`
- `CODEX_INPUT_ANSWER_SUBMITTED`
- `CODEX_INPUT_ANSWERED`
- `CODEX_INPUT_INTERRUPTED`
- `CODEX_INPUT_EXPIRED`
- `CODEX_INPUT_ANSWER_FAILED`

事件包含 module、cycle、thread、turn、input request ID 与非敏感状态原因，不记录答案之外的凭据或 ChatGPT transport 数据。

## 测试矩阵

Rust 自动化测试至少覆盖：

1. 一个匹配的 App Server input request 只创建一条 `PENDING` 记录，不创建 cycle、turn 或 thread；
2. `AnswerInput` 只响应原 request，且不调用 `turn/start`；
3. 多题 UI 可按原顺序展示，但 wire response 只以 `question.id → answers[]` 映射；不得使用显示文本、header 或数组下标；
4. 空问题产生 `[]` 而不是 `[""]`；options 仅展示，不限制自由文本；
5. worker 写出 JSON-RPC response 后仍为 `ANSWERING`；只有匹配 request ID 的 `serverRequest/resolved` 才标记 `ANSWERED`；
6. 提交前先收到 resolved、或 App Server 已 cleared 的 request 变为 `EXPIRED`，迟到提交零发送；
7. 重复提交不会第二次回答同一 request；response 已发出但 unresolved 后中断时为 `INTERRUPTED`，零自动重发；
8. App/runtime 重启把 `PENDING` 与 `ANSWERING` 标为 `INTERRUPTED` 并使 module 进入 `RECOVERY_REQUIRED`，零自动恢复；
9. secret 原值不写 SQLite、events、日志或 diagnostic；secret 提交失败/重启后零恢复或重发；
10. `autoResolutionMs` 可展示但不自行使 request 过期；
11. 等待 input 期间 runtime 继续计时；terminate/runtime budget 后可回答当前 request 并让同一 turn 自然结束，但 final result 不回 ChatGPT；
12. input answer 不写 `relay_messages`、不改变 ChatGPT FIFO、`UNKNOWN` 或 `invalid_reply_count`；
13. worker/transport failure 产生可行动错误，绝不伪造已回答状态。

React 自动化测试至少覆盖：

1. 多题输入卡按原顺序显示原问题、参考 options 和自由文本框；
2. 空回答可确认提交，secret 输入不在 UI 回显或 diagnostic 中出现；
3. 提交后显示 `ANSWERING` 与「答案已发送，正在等待 Codex 确认」，并防止重复点击；
4. `INTERRUPTED` 与 `EXPIRED` 显示中文原因且没有提交入口；
5. 终止请求后的 pending input 明确说明可完成当前输入、结果不会回传 ChatGPT；
6. ChatGPT 时间线和全局 ChatGPT 通道状态不混入 Codex input 生命周期。

## 非目标

本设计不实现对旧 request 的自动恢复、答案编辑或重发、多个并发 input request、由 ChatGPT 代答、ChatGPT `CODEX_INPUT` 控制块、强制取消 Codex turn、原始 App Server event viewer 或任何 ChatGPT Relay 协议变更以外的功能。
