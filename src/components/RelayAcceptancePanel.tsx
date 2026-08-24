import { FormEvent, useState } from 'react';

interface RelayAcceptancePanelProps {
  blockedByUnknown: boolean;
  busy: boolean;
  onAccept: () => Promise<void>;
  onSubmitFeedback: (text: string) => Promise<void>;
}

export function RelayAcceptancePanel({
  blockedByUnknown,
  busy,
  onAccept,
  onSubmitFeedback,
}: RelayAcceptancePanelProps) {
  const [feedback, setFeedback] = useState('');
  const trimmedFeedback = feedback.trim();

  async function submitFeedback(event: FormEvent) {
    event.preventDefault();
    if (!trimmedFeedback || busy) return;
    try {
      await onSubmitFeedback(trimmedFeedback);
      setFeedback('');
    } catch {
      // The workspace notice presents the backend's actionable Chinese error.
    }
  }

  async function acceptModule() {
    if (busy || blockedByUnknown) return;
    try {
      await onAccept();
    } catch {
      // The workspace notice presents the backend's actionable Chinese error.
    }
  }

  return <section className="form-section relay-acceptance-panel" aria-label="模块验收">
    <h3>等待人工验收</h3>
    <p className="execution-status">ChatGPT 已请求结束本模块。请检查代码、测试和结果。</p>
    {blockedByUnknown ? <p className="relay-action-warning">请先处理本模块的不确定送达消息</p> : null}
    <button className="primary" type="button" onClick={() => void acceptModule()} disabled={busy || blockedByUnknown}>接受并完成模块</button>
    <form className="relay-feedback-form" onSubmit={submitFeedback}>
      <label>验收反馈<textarea rows={3} value={feedback} onChange={(event) => setFeedback(event.target.value)} placeholder="说明需要继续处理或补充验证的内容。" /></label>
      {!trimmedFeedback ? <p className="relay-feedback-hint">请填写验收反馈</p> : null}
      <button className="secondary" type="submit" disabled={busy || !trimmedFeedback}>提交反馈并继续</button>
    </form>
  </section>;
}
