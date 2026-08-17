# Conversation Relay V2：模块验收与终止实施计划

> 本计划落实已批准的 [模块验收与终止设计](../specs/2026-08-18-relay-acceptance-termination-design.md)，以当前 `982e8a9` 为基线。每个任务先加入失败测试，再实施最小行为，最后运行本任务验证并独立提交。

## 目标

实现用户对 relay module 的验收通过、验收反馈继续和安全终止，使终态模块不会再被 FIFO 或迟到异步事件恢复为活动状态，同时保持 ChatGPT 控制协议、全局 FIFO、`UNKNOWN` 显式恢复和单 Codex runtime 语义不变。

## 当前代码定位与约束

- `src-tauri/src/lib.rs`
  - `claim_next_relay_message_for_dispatch` 当前从所有 `TO_CHATGPT/QUEUED` 选择消息，未排除终态模块。
  - `queue_relay_message` 已拒绝 `STOPPED`/`COMPLETED` 的新消息。
  - `handle_relay_chatgpt_reply` 目前在保存 reply 后总会对自动化消息解析 terminal control block。
  - `RelayCodexCommand` 只有 `StartTurn`，`relay_codex_worker` 仅在循环退出时 kill/wait child；没有 release acknowledgement。
  - `relay_codex_turn_completed` 总是持久化 final text、追加 `FROM_CODEX`、排队 result，并调用 dispatcher。
  - `stop_after_turn` 已存在于 `relay_modules` 和 `RelayModuleRecord`，但当前没有行为消费者。
  - 当前 relay phase 没有 `WAITING_FOR_CHATGPT` 字符串。Task 4 将在反馈事务中引入这个已批准的最小 phase，并将所有依赖 phase 的终态/启动检查同步处理；不改变数据库 schema，因为 phase 当前是自由文本。
- `src/App.tsx` 目前在选中模块时始终渲染 composer，且没有验收或终止 action。
- `src/App.test.tsx` 已有 Tauri invoke mock 和 observability 组件测试，可扩展模块 action 集成测试。
- `src/styles.css` 是现有无依赖 CSS；不引入新的 UI framework。
- `src-tauri/migrations/004_conversation_relay_v2.sql` 已含 `stop_after_turn`；本计划无需新增 cycle 状态或数据库 schema。

不实现 `CODEX_INPUT`、released-thread resume、browser-history sync、多 Codex 并发、强杀运行 turn、新控制块或 browser adapter 变更。

## Task 1：终态保护与 FIFO 防御

**文件：** `src-tauri/src/lib.rs`

1. 在 Rust tests 中先建立 `COMPLETED` 和 `STOPPED` 模块，各插入 `TO_CHATGPT/QUEUED`，断言 `claim_next_relay_message_for_dispatch` 不会 claim 它们。
2. 在同一测试组加入活动模块跨模块 FIFO 用例，断言其仍按现有 `created_at, id` 排序被 claim。
3. 加入 `queue_relay_message` 对两个终态均拒绝的回归测试。
4. 加入旧/防御数据用例：终态模块的一条 `UNKNOWN` 经现有恢复路径处理时，`set_relay_phase_after_recovery` 不得把该模块写回 `READY`。
5. 运行 `cargo test terminal_relay -- --nocapture`，确认实现前 FIFO 或 phase 回退断言失败。
6. 修改 `next_queued_relay_message`/claim 查询：`relay_messages JOIN relay_modules`，仅选择 `module.phase NOT IN ('COMPLETED', 'STOPPED')` 的 `QUEUED`。
7. 修改 `set_relay_phase_after_recovery`：先读当前 phase；若为终态则不写 phase，仍保持消息恢复的既有显式语义。该分支只保护遗留状态，不自动解决 `UNKNOWN`。
8. 保持 dispatcher 的全局 FIFO、`UNKNOWN > SENT` 优先级和非终态消息行为不变。
9. 运行目标测试、`cargo test`、`cargo check`。
10. 提交：`fix: guard terminal relay modules`

## Task 2：最小 Codex runtime release 生命周期

**文件：** `src-tauri/src/lib.rs`

1. 先为可测试的 release 协调 helper 写 Rust tests：idle session release 会收到 acknowledgement、仅清除匹配模块的 `state.relay_codex`、不会清除数据库 `codex_thread_id`、重复 release 无副作用、释放后不同模块可创建 session。
2. 为运行 turn 写负向测试：普通 release helper 不向运行 worker 发送 kill/Release；运行中终止将在 Task 6 使用 `stop_after_turn`。
3. 运行 `cargo test relay_codex_release -- --nocapture`，确认缺少 release API 时失败。
4. 将 `RelayCodexCommand` 扩展为：
   ```rust
   Release { acknowledgement: std_mpsc::Sender<Result<(), String>> }
   ```
5. 增加 `release_relay_codex_runtime(app, module_id)`：先短暂取得 `relay_codex` mutex 复制匹配 sender，立即释放 mutex，再发送 Release 并以有界等待读取 acknowledgement；不得在持有 `relay_codex` mutex 或 DB lock 时等待 worker。
6. worker 收到 Release 时仅在没有 active/pending turn 时退出；退出路径关闭 stdin、结束 child、`wait` 后发送 acknowledgement。worker 退出后用模块 ID 条件清空 `state.relay_codex`，防止清除新模块 session。
7. 对没有 session 的模块把 release 视为成功无操作；对 worker 已断开或 acknowledgement 失败返回明确错误，不在 helper 中改变 module phase。
8. 保留 `relay_modules.codex_thread_id`，不实现 `thread/resume` 或通用进程管理。
9. 运行目标测试、`cargo test`、`cargo check`。
10. 提交：`feat: release relay Codex runtime`

## Task 3：接受并完成模块

**文件：** `src-tauri/src/lib.rs`

1. 先添加 `accept_relay_module(module_id)` 的 Rust tests：
   - 仅 `WAITING_FOR_ACCEPTANCE` 可接受；
   - 本模块 `UNKNOWN` 拒绝，其他模块 `UNKNOWN` 不阻止；
   - 运行 `CODEX_RUNNING` cycle 拒绝；
   - accept 后 `COMPLETED`，queued 原文保留但都为 `FAILED`，sent 不变，`codex_thread_id` 保留；
   - `COMPLETED` 后新消息被拒绝；
   - 重复 accept 不重复 `MODULE_ACCEPTED`/`CODEX_THREAD_RELEASED`；`STOPPED` accept 拒绝。
2. 运行 `cargo test accept_relay_module -- --nocapture`，确认 command 尚不存在而失败。
3. 增加 `accept_relay_module` Tauri command。数据库事务内完成：读取 module、验证 phase/`stop_after_turn`/本模块 unknown/运行 cycle、将本模块所有 `TO_CHATGPT/QUEUED` 更新为 `FAILED`、记录每条未发送消息审计、写入 `COMPLETED`、追加一次 `MODULE_ACCEPTED`。
4. 事务提交后调用 Task 2 release helper；成功后追加一次 `CODEX_THREAD_RELEASED`。release 错误只记录 `CODEX_THREAD_RELEASE_FAILED` 并以中文错误返回，module 仍保持 `COMPLETED`，绝不恢复自动化。
5. 在 `generate_handler!` 注册 command；使用现有 `relay-control`/`relay-codex` 刷新事件，使前端无需刷新页面。
6. 运行目标测试、`cargo test`、`cargo check`。
7. 提交：`feat: accept completed relay modules`

## Task 4：验收反馈继续

**文件：** `src-tauri/src/lib.rs`

1. 先添加 `submit_relay_acceptance_feedback(module_id, text)` tests：
   - 仅 `WAITING_FOR_ACCEPTANCE` 且 trim 后非空允许；
   - 成功仅插入一条 `TO_CHATGPT/AUTOMATION/QUEUED`，使用既有 sequence 与 `CHATGPT_MESSAGE_QUEUED` 事件；
   - 不调用手动消息/直接 WebSocket helper；
   - 首次提交后 module 为 `WAITING_FOR_CHATGPT`，重复提交拒绝且不插入第二条；
   - 本模块或其他模块 `UNKNOWN` 时反馈仍可安全入队，但 dispatcher 不得自动越过全局 blocker；
   - 后续对该消息的有效 `CODEX_PROMPT` 复用既有 thread 且 `started_cycles` 增加。
2. 运行 `cargo test relay_acceptance_feedback -- --nocapture`，确认失败。
3. 增加 Tauri command，在一个事务中验证、插入 automation queue message、写入 `WAITING_FOR_CHATGPT` 和 `ACCEPTANCE_FEEDBACK_QUEUED` 事件；提交后调用现有 `dispatch_next_relay_message`。
4. 在 `handle_relay_chatgpt_reply` 与 `start_or_continue_relay_codex_turn` 检查 phase：`WAITING_FOR_CHATGPT` 是等待自动化回复的非终态，匹配 reply 后可以按现有 parser 正常进入 `CODEX_PROMPT_READY`/新 turn；不能被终态 gate 误拦截。
5. 明确处理所有 phase 依赖：`queue_relay_message` 继续只禁止 `COMPLETED`/`STOPPED`；module 列表/UI 原样展示新 phase；snapshot/FIFO 不以该 phase 排除消息。
6. 运行目标测试、`cargo test`、`cargo check`。
7. 提交：`feat: continue relay from acceptance feedback`

## Task 5：终止模块——无运行 Codex turn

**文件：** `src-tauri/src/lib.rs`

1. 先为 `terminate_relay_module(module_id)` 的 non-running 路径写测试：`READY`、`WAITING_FOR_ACCEPTANCE`、`BLOCKED`、`RECOVERY_REQUIRED`、`WAITING_FOR_CHATGPT` 都转 `STOPPED`；本模块 unknown 拒绝，其他模块 unknown 不阻止；queued 变 `FAILED` 且保留原文/审计；sent 不变；thread ID 保留；重复 stop 无副作用；completed stop 拒绝；stopped 拒绝新消息。
2. 运行 `cargo test terminate_idle_relay_module -- --nocapture`，确认失败。
3. 新增 command 的 idle 分支，事务内验证非终态、当前模块无 unknown、没有 `CODEX_RUNNING` cycle/active session，批量 queued → failed，写 `STOPPED` 和 `MODULE_TERMINATED`。
4. 事务提交后调用 Task 2 release helper；成功追加 `CODEX_THREAD_RELEASED`，失败追加 `CODEX_THREAD_RELEASE_FAILED` 并保持 `STOPPED`。
5. 不能将任何 `SENT` 转为 `FAILED`，也不能把 `UNKNOWN` 作为终止副作用解决。
6. 注册 command 和刷新事件。
7. 运行目标测试、`cargo test`、`cargo check`。
8. 提交：`feat: terminate idle relay modules`

## Task 6：终止模块——Codex turn 正在运行

**文件：** `src-tauri/src/lib.rs`

1. 先增加运行 turn 终止测试：
   - `CODEX_RUNNING` terminate 只设 `stop_after_turn = 1`，不立即 `STOPPED`，不发送 Release/kill；
   - 重复 terminate 不重复事件；
   - 已请求停止时新的 `CODEX_PROMPT` 不会启动 turn；
   - completion 保存 `result_text`、`codex_completed_at`、`FROM_CODEX`，cycle 保持 `CODEX_COMPLETED`，不创建 outbound message/`TO_CHATGPT` result、不调用 dispatcher；
   - completion 后 module `STOPPED`、flag 复位、release、thread ID 保留；
   - 实际 turn failure 后 cycle 可为 `FAILED`，但 module 最终 `STOPPED`；
   - completion/termination interleaving 最多产生一个收尾路径。
2. 运行 `cargo test terminate_running_relay_codex -- --nocapture`，确认失败。
3. 在 `terminate_relay_module` 检测 active worker 或 `CODEX_RUNNING` cycle 时走 running 分支：同一事务内设置 `stop_after_turn = 1`、将既有 queued 标为 failed、追加 `MODULE_STOP_REQUESTED`；不 release。
4. 在 `start_or_continue_relay_codex_turn` 的数据库前置检查中拒绝 `stop_after_turn = 1`、`STOPPED` 和 `COMPLETED`，并只将尚未启动的 cycle 记录为明确不可启动，不创建新 turn。
5. 修改 `relay_codex_turn_completed`：先持久化 final text 与 `FROM_CODEX`；读取 `stop_after_turn`。为真时不调用 `queue_relay_codex_result_to_chatgpt`、不创建 result outbound、不调用 dispatcher；将 module 原子收尾为 `STOPPED`、复位 flag，之后调用 release helper。
6. 修改 `relay_codex_failed`：若 stop intent 已存在，保留真实 cycle failure 信息但收尾 module 为 `STOPPED`，不改回 `BLOCKED`，之后 release。
7. 修改 `list_relay_codex_cycles_in` 的结构化 `block_reason` 计算：`STOPPED` 模块中 `CODEX_COMPLETED` 且无 outbound message 显示「模块已由用户终止，结果未回传 ChatGPT」。不新增 cycle status，不写伪造 error。
8. 运行目标测试、`cargo test`、`cargo check`。
9. 提交：`feat: stop relay after active Codex turn`

## Task 7：终态迟到 ChatGPT reply

**文件：** `src-tauri/src/lib.rs`

1. 先添加 STOPPED/COMPLETED 的 matching `SENT` reply tests，分别覆盖自动化和手动消息，以及回复中包含有效 `CODEX_PROMPT` 的场景。断言：`SENT → DELIVERED`、真实 `FROM_CHATGPT` 文本保存、关联 Codex cycle 正常送达同步、module phase 不变、没有新 cycle/turn/retry。
2. 运行 `cargo test terminal_relay_chatgpt_reply -- --nocapture`，确认失败。
3. 调整 `handle_relay_chatgpt_reply` 顺序：匹配 request ID、完成 delivery/cycle correlation、保存 `FROM_CHATGPT` 和 reply audit；然后读取 module phase。
4. 当 phase 为 `STOPPED` 或 `COMPLETED` 时追加 `LATE_CHATGPT_REPLY_IGNORED`，跳过 manual/automation 分支、terminal parser、invalid count、retry、cycle 创建和 `start_or_continue_relay_codex_turn`，然后安全继续全局 dispatcher。
5. 非终态 reply 的手动/自动化逻辑保持不变。
6. 运行目标测试、`cargo test`、`cargo check`。
7. 提交：`fix: ignore automation from terminal relay replies`

## Task 8：验收/终止 UI 与完整回归

**文件：**

- 新建 `src/components/RelayAcceptancePanel.tsx`
- 新建 `src/components/RelayModuleActions.tsx`
- 修改 `src/App.tsx`
- 修改 `src/App.test.tsx`
- 修改 `src/styles.css`

1. 先写 React 组件和 App 集成失败测试：
   - `WAITING_FOR_ACCEPTANCE` 显示「等待人工验收」、接受、反馈、终止三种 action；
   - accept 调 `accept_relay_module`；空 feedback 不提交，成功后清空；
   - terminate 需要二次确认；
   - `CODEX_RUNNING + stopAfterTurn` 显示「终止已请求，等待当前 Codex 回合结束」且禁用终止；
   - `COMPLETED` 显示「已验收完成」并隐藏 composer/actions；`STOPPED` 显示「已终止」并隐藏 composer/actions；
   - 当前模块 unknown 时 accept/terminate 显示先恢复提示；后端拒绝显示中文 notice；
   - ChatGPT `.message-history` 不出现控制事件或 synthetic lifecycle row。
2. 运行 `npm test -- --run`，确认新增断言在组件/command 接线前失败。
3. 实现 `RelayAcceptancePanel`：受控 feedback textarea，调用 `submit_relay_acceptance_feedback`；实现 `RelayModuleActions`：非终态 terminate、`window.confirm` 二次确认、busy 防重复。不得添加 UI framework。
4. 在 `App.tsx` 增加 action callbacks，调用三个后端 product commands。每个成功或失败 action 后刷新 modules、messages、recovery messages、codex cycles、channel snapshot；使用现有 notice 显示后端中文错误。
5. 根据 module phase/`stopAfterTurn` 条件渲染：验收卡仅等待验收、一般终止仅非终态、终态无 composer/终止/验收输入。保留既有 global recovery panel 与 Codex observability panel。
6. 为新组件添加最小 CSS：验收卡、危险终止按钮、终态提示、stop-requested 文案以及移动端可读布局。`<pre>`、消息时间线和现有 Chinese UI 风格不改。
7. 运行 `npm test -- --run`、`npm run build`、`cargo test`、`cargo check`。
8. 提交：`feat: add relay acceptance controls`

## 完整验证与人工交接

Task 1–8 完成后，在仓库根目录运行：

```powershell
node .\spikes\chatgpt-extension\protocol-text.test.mjs
node .\spikes\chatgpt-extension\adapter-version.test.mjs
node .\spikes\chatgpt-extension\background-relay.test.mjs

npm test -- --run
npm run build

Set-Location src-tauri
cargo test
cargo check
Set-Location ..
```

若自动化未发现缺陷，不创建额外验证提交。真实 Chrome/Tauri 验收只有在当前环境实际完成时才报告通过；否则人工验证应覆盖：`MODULE_DONE` 验收卡、反馈进入 automation FIFO、idle terminate、running terminate 等待自然收尾、终态迟到 reply 保存但不自动化、当前模块 `UNKNOWN` 阻止结束、其他模块 `UNKNOWN` 不阻止本模块结束。

## 计划自检

- 每个 Task 都以失败测试开始，并以目标、全量 Rust 或前端验证结束。
- 每个 Task 有独立、可 review 的最小提交。
- 所有状态转换在后端 transaction/lock 中检查；前端 disabled 不作为安全边界。
- 未新增 ChatGPT 协议、cycle 状态、Codex 并发或隐式 `UNKNOWN` 恢复。
- 终态 phase 在 dispatcher、worker、reply handler 和 UI 四层均被保护，不会被迟到异步事件恢复。
