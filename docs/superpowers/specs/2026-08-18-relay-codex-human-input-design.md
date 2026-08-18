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
- ChatGPT FIFO、`UNKNOWN` recovery、automation terminal-control parser 和 automation retry 不参与本流程。
- 不增加多 Codex 并发、不强杀 turn、不编辑或重发 Codex prompt，也不新增 ChatGPT 控制块。

## 术语

| 术语 | 含义 |
| --- | --- |
| input request | Codex App Server 在既有 turn 内发出的、要求用户提供一个或多个答案的 request。 |
| pending request | 状态为 `PENDING`，仍可由用户回答的 input request。每个活跃 Codex turn 最多一个。 |
| original request | 由 App Server 赋予的 request ID、所属 thread/turn 和问题顺序；回答必须回到这一 request，不能构造新 request。 |
| input session | middleware 对一个 `PENDING` request 的本地持久化与 UI 展示，不是新的 Codex thread 或 relay cycle。 |

## 用户流程

```text
Codex App Server user-input request
  -> middleware 验证其属于当前 active worker / thread / turn
  -> 原子持久化 relay_codex_input_requests = PENDING
  -> module 保持原运行 phase，UI 显示“等待你的 Codex 输入”
  -> 用户逐题输入自由文本并提交
  -> middleware 原子 claim 该 PENDING request
  -> worker AnswerInput 响应原 App Server request
  -> 成功：request = ANSWERED，turn 继续运行
  -> App Server 清除/拒绝该 request：request = EXPIRED，禁止迟到提交
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
| `questions_json` | 按 App Server 原始顺序持久化的问题数组；每项包含题目文本、可选 options 和顺序号。 |
| `answers_json` | 用户提交的自由文本答案，按问题顺序；`PENDING` 时为空。 |
| `status` | `PENDING`、`ANSWERING`、`ANSWERED`、`INTERRUPTED` 或 `EXPIRED`。 |
| `error_text` | 非敏感、可行动的失败或过期原因；无错误时为空。 |
| `created_at` | 收到 App Server request 的时间。 |
| `answered_at` | App Server 成功接受答案的时间；否则为空。 |
| `interrupted_at` | 重启或答案送达结果不确定时标记中断的时间；否则为空。 |
| `expired_at` | App Server 已 resolved/cleared 或迟到提交被拒绝时的时间；否则为空。 |
| `updated_at` | 最后一次状态转换时间。 |

问题和答案使用 JSON 仅保存结构化的本地输入，不作为 ChatGPT 协议载荷。保存时不得记录 cookie、pairing secret、密码或其他凭据。

### 状态机

| 当前状态 | 触发 | 下一状态 | 规则 |
| --- | --- | --- |
| 无记录 | 收到匹配 active turn 的 App Server request | `PENDING` | 原子写入问题、原 request ID 与审计事件；不创建 cycle/turn/thread。 |
| `PENDING` | 用户提交有效答案 | `ANSWERING` | 同一事务 claim；答案数量必须与已持久化问题数量相同，每项按用户输入原样作为自由文本提交，包括空文本。 |
| `ANSWERING` | 原 App Server request 接受答案 | `ANSWERED` | 持久化答案与时间，turn 继续。 |
| `ANSWERING` | App Server 表明 request 已 resolved/cleared、或提交已迟到 | `EXPIRED` | 保存原因；不得重新发送或改回 `PENDING`。 |
| `ANSWERING` | worker 或本地 App Server transport 无法确认答案是否被接受 | `INTERRUPTED` | module 进入 `RECOVERY_REQUIRED`；不得自动重发答案或假定 request 仍可写。 |
| `PENDING` / `ANSWERING` | App/runtime 重启 | `INTERRUPTED` | 模块进入 `RECOVERY_REQUIRED`；不得自动恢复、重放或再次回答旧 request。 |
| `PENDING` / `ANSWERING` | App Server request 已被其他路径 resolved/cleared | `EXPIRED` | 禁止 UI 迟到提交。 |

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

发送该命令前，后端必须在事务内验证：模块非终态；input request 是当前 module/cycle/thread/turn 的 `PENDING` 记录；其 `app_server_request_id` 与当前 worker request 相同；答案顺序与持久化问题一致；且该 request 尚未被 App Server cleared。worker 只把答案写回原 App Server request，并以 acknowledgement 报告 accepted、expired 或 transport failure。

`AnswerInput` 不得调用 `turn/start`、不得创建 `RelayCodexSession`、不得改变 `started_cycles`、不得更新 `codex_thread_id`，也不得排队任何 ChatGPT 消息。worker 在收到 App Server input request 后继续持有同一 turn；收到答案后继续等待同一 turn 的后续事件和自然完成。

建议新增 Tauri commands：

- `list_relay_codex_input_requests(module_id)`：返回模块的输入 request 历史和当前状态；不返回任何 ChatGPT transport 状态。
- `submit_relay_codex_input(input_request_id, answers)`：原子 claim 并向匹配 worker 发送 `AnswerInput`；返回 pending、accepted、expired 或 interrupted 的结构化中文结果。

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

若 App Server 明确报告原 request 已 resolved、cleared、unknown 或 request ID 不再匹配，记录改为 `EXPIRED`。UI 必须移除提交动作，保留问题、已保存答案（如有）和原因以便审计。迟到的 `submit_relay_codex_input` 永远不发送到任何 App Server request。

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

卡片显示所属 cycle number、thread ID、turn ID、request 状态和阻塞原因。`ANSWERING` 禁止重复提交；`INTERRUPTED` 显示「输入请求因应用或运行时重启中断，请检查模块恢复状态」；`EXPIRED` 显示 App Server 已不再接受该 request 的原因。options 不做单选、多选或必填约束。

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
3. 多题答案按原顺序提交；options 仅展示，不限制自由文本；
4. 重复提交不会第二次回答同一 request；
5. App Server 已 cleared 的 request 变为 `EXPIRED`，迟到提交零发送；
6. App/runtime 重启把 `PENDING` 与 `ANSWERING` 标为 `INTERRUPTED` 并使 module 进入 `RECOVERY_REQUIRED`，零自动恢复；
7. 等待 input 期间 runtime 继续计时；terminate/runtime budget 后可回答当前 request 并让同一 turn 自然结束，但 final result 不回 ChatGPT；
8. input answer 不写 `relay_messages`、不改变 ChatGPT FIFO、`UNKNOWN` 或 `invalid_reply_count`；
9. worker/transport failure 产生可行动错误，绝不伪造已回答状态。

React 自动化测试至少覆盖：

1. 多题输入卡显示原问题、参考 options 和自由文本框；
2. 提交期间显示 `ANSWERING` 并防止重复点击；
3. `INTERRUPTED` 与 `EXPIRED` 显示中文原因且没有提交入口；
4. 终止请求后的 pending input 明确说明可完成当前输入、结果不会回传 ChatGPT；
5. ChatGPT 时间线和全局 ChatGPT 通道状态不混入 Codex input 生命周期。

## 非目标

本设计不实现对旧 request 的自动恢复、答案编辑或重发、多个并发 input request、由 ChatGPT 代答、ChatGPT `CODEX_INPUT` 控制块、强制取消 Codex turn、原始 App Server event viewer 或任何 ChatGPT Relay 协议变更以外的功能。
