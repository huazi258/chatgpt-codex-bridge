import type { RelayRecoveryMessage } from '../relay-ui';
import './human-intervention-drawer.css';

interface HumanInterventionDrawerProps {
  kind: 'blocked' | 'acceptance';
  collapsed: boolean;
  busy: boolean;
  blockedReason: string | null;
  feedback: string;
  recoveryMessages: RelayRecoveryMessage[];
  onCollapsedChange: (collapsed: boolean) => void;
  onClose: () => void;
  onFeedbackChange: (text: string) => void;
  onSubmitBlocked: () => void;
  onAccept: () => void;
  onStartAcceptanceFeedback: () => void;
  onBackToAcceptance: () => void;
  acceptanceFeedbackOpen: boolean;
  onSubmitAcceptanceFeedback: () => void;
}

export function HumanInterventionDrawer(props: HumanInterventionDrawerProps) {
  const blocked = props.kind === 'blocked';
  const title = blocked ? '需要人工处理' : '等待人工验收';
  if (props.collapsed) return <aside className="human-intervention-drawer collapsed" aria-label={title}>
    <button className="icon-button" type="button" aria-label="展开人工处理" onClick={() => props.onCollapsedChange(false)}>‹</button>
    <button className="icon-button" type="button" aria-label="关闭人工处理" onClick={props.onClose}>×</button>
  </aside>;
  return <aside className="human-intervention-drawer" aria-label={title}>
    <header className="intervention-heading"><div><strong>{title}</strong>{blocked ? <small>Action Required</small> : <small>Human Intervention</small>}</div><div><button className="icon-button" type="button" aria-label="收起人工处理" onClick={() => props.onCollapsedChange(true)}>›</button><button className="icon-button" type="button" aria-label="关闭人工处理" onClick={props.onClose}>×</button></div></header>
    <div className="intervention-content">
      {blocked ? <>
        <p>ChatGPT 暂停了当前自动流程，需要你的输入后才能继续。</p>
        <section><h3>原因</h3><pre>{props.blockedReason ?? '正在读取原因…'}</pre></section>
        <label>回复 ChatGPT<textarea value={props.feedback} onChange={(event) => props.onFeedbackChange(event.target.value)} placeholder="输入你的回复…" rows={6} /></label>
        <div className="drawer-actions"><button type="button" onClick={props.onClose}>稍后处理</button><button className="primary" type="button" disabled={props.busy || !props.feedback.trim()} onClick={props.onSubmitBlocked}>提交并继续</button></div>
      </> : props.acceptanceFeedbackOpen ? <>
        <p>请说明需要继续处理或补充验证的内容。</p>
        <label>验收反馈<textarea value={props.feedback} onChange={(event) => props.onFeedbackChange(event.target.value)} placeholder="输入反馈…" rows={6} /></label>
        <div className="drawer-actions"><button type="button" onClick={props.onBackToAcceptance}>返回</button><button className="primary" type="button" disabled={props.busy || !props.feedback.trim()} onClick={props.onSubmitAcceptanceFeedback}>提交并继续</button></div>
      </> : <>
        <p>ChatGPT 已请求结束当前会话。请检查代码、测试和执行结果。</p>
        {props.recoveryMessages.length ? <p className="message-warning">请先处理本会话的不确定送达消息。</p> : null}
        <div className="drawer-actions"><button type="button" onClick={props.onStartAcceptanceFeedback}>继续处理</button><button className="primary" type="button" disabled={props.busy || props.recoveryMessages.length > 0} onClick={props.onAccept}>接受并完成</button></div>
      </>}
    </div>
  </aside>;
}
