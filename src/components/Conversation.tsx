import { FormEvent } from 'react';
import type { RelayCodexCycle } from '../relay-observability';
import { codexCycleStatusLabel } from '../relay-observability';
import { phaseLabel, terminalPhases, type RelayKind, type RelayMessage, type RelayModule, type RelayRecoveryMessage } from '../relay-ui';

interface ConversationProps {
  session: RelayModule;
  messages: RelayMessage[];
  cycles: RelayCodexCycle[];
  recoveryMessages: RelayRecoveryMessage[];
  notice: string;
  text: string;
  kind: RelayKind;
  busy: boolean;
  onTextChange: (text: string) => void;
  onKindChange: (kind: RelayKind) => void;
  onSend: (event: FormEvent) => void;
  onRetryUnknown: (messageId: string) => void;
  onContinueUnknown: (messageId: string) => void;
}

export type ConversationTimelineItem =
  | { type: 'message'; message: RelayMessage; cycle?: RelayCodexCycle }
  | { type: 'forwarded-codex-result'; message: RelayMessage }
  | { type: 'cycle-status'; cycle: RelayCodexCycle };

/** Message sequence is the authoritative order; cycle records only add status. */
export function buildConversationTimeline(messages: RelayMessage[], cycles: RelayCodexCycle[]): ConversationTimelineItem[] {
  const orderedMessages = [...messages].sort((left, right) => left.sequenceNumber - right.sequenceNumber);
  const orderedCycles = [...cycles].sort((left, right) => left.cycleNumber - right.cycleNumber);
  const items: ConversationTimelineItem[] = [];
  let nextCycle = 0;
  let lastCodexResult: string | null = null;

  for (const message of orderedMessages) {
    if (message.direction === 'TO_CODEX') {
      items.push({ type: 'message', message, cycle: orderedCycles[nextCycle++] });
      continue;
    }
    if (message.direction === 'TO_CHATGPT' && message.kind === 'AUTOMATION' && lastCodexResult === message.text) {
      items.push({ type: 'forwarded-codex-result', message });
      continue;
    }
    items.push({ type: 'message', message });
    if (message.direction === 'FROM_CODEX') lastCodexResult = message.text;
  }

  // Without a TO_CODEX record, a cycle cannot be placed reliably. Keep only its
  // status visible, never a duplicate prompt/result body.
  for (; nextCycle < orderedCycles.length; nextCycle += 1) items.push({ type: 'cycle-status', cycle: orderedCycles[nextCycle] });
  return items;
}

function messageTitle(message: RelayMessage): string {
  switch (message.direction) {
    case 'TO_CHATGPT': return message.kind === 'MANUAL' ? '你 → ChatGPT' : '自动化 → ChatGPT';
    case 'FROM_CHATGPT': return 'ChatGPT';
    case 'TO_CODEX': return 'ChatGPT → Codex Prompt';
    case 'FROM_CODEX': return 'Codex';
  }
}

function CycleDivider({ cycle }: { cycle: RelayCodexCycle }) {
  return <><div className="cycle-divider"><span>第 {cycle.cycleNumber} 轮 · Cycle {cycle.cycleNumber}</span></div><p className={`inline-system ${cycle.status === 'FAILED' ? 'failed' : ''}`}>{cycle.status === 'CODEX_COMPLETED' ? '✓ Codex 已完成本轮任务' : codexCycleStatusLabel(cycle.status)}{cycle.errorText ? `：${cycle.errorText}` : ''}</p></>;
}

export function Conversation(props: ConversationProps) {
  const sessionRecovery = props.recoveryMessages.filter((message) => message.moduleId === props.session.id);
  const timeline = buildConversationTimeline(props.messages, props.cycles);
  return <section className="conversation-workspace" aria-label="ChatGPT 与 Codex 会话">
    <div className="conversation-scroll" aria-live="polite">
      <p className="notice compact" role="status">{props.notice}</p>
      {sessionRecovery.map((message) => <article className="inline-system unknown" key={message.messageId}>
        <div><strong>送达结果不确定</strong><span>第 {message.sequenceNumber} 条 {message.kind === 'MANUAL' ? '聊天' : '自动化'}消息不会自动重发。</span></div>
        <div><button type="button" disabled={props.busy} onClick={() => props.onRetryUnknown(message.messageId)}>明确重发</button><button type="button" disabled={props.busy} onClick={() => props.onContinueUnknown(message.messageId)}>不重发并继续</button></div>
      </article>)}
      {!timeline.length ? <div className="conversation-empty"><span>✦</span><h2>开始一段受控会话</h2><p>发送聊天消息，或切换到自动化以请求 ChatGPT 输出控制块。</p></div> : null}
      {timeline.map((item) => {
        if (item.type === 'cycle-status') return <section className="automation-step" key={item.cycle.id}><CycleDivider cycle={item.cycle} /></section>;
        if (item.type === 'forwarded-codex-result') return <p className="inline-system" key={item.message.id}>Codex 结果已回传 ChatGPT</p>;
        const { message, cycle } = item;
        return <section className="automation-step" key={message.id}>
          {cycle ? <CycleDivider cycle={cycle} /> : null}
          <article className={`timeline-message ${message.direction.toLowerCase()}`}>
            <header><span className="message-role">{messageTitle(message)}</span><span>{message.kind === 'MANUAL' ? '聊天' : message.kind === 'AUTOMATION' ? '自动化' : '系统'} · {message.deliveryState}</span></header>
            <pre>{message.text}</pre>
            {message.direction === 'TO_CHATGPT' && message.deliveryState === 'UNKNOWN' ? <p className="message-warning">送达结果不确定，等待你的明确处理。</p> : null}
          </article>
        </section>;
      })}
      {props.session.phase === 'WAITING_FOR_ACCEPTANCE' ? <p className="inline-system">等待人工验收</p> : null}
      {props.session.phase === 'WAITING_FOR_CHATGPT' ? <p className="inline-system">正在等待 ChatGPT 回复…</p> : null}
      {terminalPhases.has(props.session.phase) ? <p className="inline-system">会话{phaseLabel(props.session.phase)}，历史记录仍可查看。</p> : null}
    </div>
    {!terminalPhases.has(props.session.phase) ? <form className="conversation-composer" onSubmit={props.onSend}>
      <div className="composer-toolbar"><label>模式<select aria-label="消息模式" value={props.kind} onChange={(event) => props.onKindChange(event.target.value as RelayKind)}><option value="MANUAL">聊天</option><option value="AUTOMATION">自动化</option></select></label>{props.kind === 'AUTOMATION' ? <small>此回复将参与自动化控制。</small> : null}</div>
      <textarea value={props.text} rows={3} onChange={(event) => props.onTextChange(event.target.value)} placeholder="输入消息…" />
      <button className="primary send-button" disabled={props.busy} type="submit">{props.busy ? '正在处理…' : '发送'}</button>
    </form> : null}
  </section>;
}
