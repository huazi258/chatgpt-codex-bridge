interface RelayModuleActionsProps {
  phase: string;
  stopAfterTurn: boolean;
  blockedByUnknown: boolean;
  busy: boolean;
  onTerminate: () => Promise<void>;
}

const terminalPhases = new Set(['COMPLETED', 'STOPPED']);

export function RelayModuleActions({
  phase,
  stopAfterTurn,
  blockedByUnknown,
  busy,
  onTerminate,
}: RelayModuleActionsProps) {
  if (terminalPhases.has(phase)) return null;

  const terminationRequested = phase === 'CODEX_RUNNING' && stopAfterTurn;

  async function terminateModule() {
    if (busy || blockedByUnknown || terminationRequested) return;
    const warning = phase === 'CODEX_RUNNING'
      ? '当前 Codex 回合不会被强制停止；它自然结束后，结果不会回传 ChatGPT，模块将终止。确定要终止模块吗？'
      : '确定要终止模块吗？尚未发送的消息将不会发送。';
    if (!window.confirm(warning)) return;
    try {
      await onTerminate();
    } catch {
      // The workspace notice presents the backend's actionable Chinese error.
    }
  }

  return <section className="form-section relay-module-actions" aria-label="模块操作">
    <h3>模块操作</h3>
    {terminationRequested ? <p className="stop-requested">终止已请求，等待当前 Codex 回合结束</p> : null}
    {blockedByUnknown ? <p className="relay-action-warning">请先处理本模块的不确定送达消息</p> : null}
    <button
      className="danger"
      type="button"
      disabled={busy || blockedByUnknown || terminationRequested}
      onClick={() => void terminateModule()}
    >终止模块</button>
  </section>;
}
