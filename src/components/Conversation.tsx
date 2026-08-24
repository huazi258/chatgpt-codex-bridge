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
  | { type: 'cycle-divider'; cycle: RelayCodexCycle }
  | { type: 'message'; message: RelayMessage }
  | { type: 'forwarded-codex-result'; message: RelayMessage }
  | { type: 'cycle-status'; cycle: RelayCodexCycle };

/** Message sequence is the authoritative order; cycle records only add status. */
export function buildConversationTimeline(messages: RelayMessage[], cycles: RelayCodexCycle[]): ConversationTimelineItem[] {
  const orderedMessages = [...messages].sort((left, right) => left.sequenceNumber - right.sequenceNumber);
  const orderedCycles = [...cycles].sort((left, right) => left.cycleNumber - right.cycleNumber);
  const cycleByOutboundMessageId = new Map(
    orderedCycles.flatMap((cycle) => cycle.outboundChatgptMessageId ? [[cycle.outboundChatgptMessageId, cycle] as const] : []),
  );
  const cycleByPromptMessageId = new Map<string, RelayCodexCycle>();
  let promptCycleIndex = 0;
  for (const message of orderedMessages) {
    if (message.direction === 'TO_CODEX' && orderedCycles[promptCycleIndex]) cycleByPromptMessageId.set(message.id, orderedCycles[promptCycleIndex++]);
  }
  const resultCycleByMessageId = new Map<string, RelayCodexCycle>();
  const promptCycles = new Set(cycleByPromptMessageId.values());
  for (const message of orderedMessages) {
    if (message.direction !== 'FROM_CODEX') continue;
    const cycle = orderedCycles.find((candidate) => promptCycles.has(candidate) && candidate.resultText === message.text && ![...resultCycleByMessageId.values()].includes(candidate));
    if (cycle) resultCycleByMessageId.set(message.id, cycle);
  }
  const items: ConversationTimelineItem[] = [];

  for (const message of orderedMessages) {
    if (message.direction === 'TO_CODEX') {
      const cycle = cycleByPromptMessageId.get(message.id);
      if (cycle) items.push({ type: 'cycle-divider', cycle });
      items.push({ type: 'message', message });
      if (cycle && ![...resultCycleByMessageId.values()].includes(cycle)) items.push({ type: 'cycle-status', cycle });
      continue;
    }
    if (message.direction === 'TO_CHATGPT' && cycleByOutboundMessageId.has(message.id)) {
      items.push({ type: 'forwarded-codex-result', message });
      continue;
    }
    items.push({ type: 'message', message });
    const cycle = resultCycleByMessageId.get(message.id);
    if (cycle) items.push({ type: 'cycle-status', cycle });
  }

  // Without a TO_CODEX record, a cycle cannot be placed reliably. Keep only its
  // status visible, never a duplicate prompt/result body.
  for (const cycle of orderedCycles) if (!promptCycles.has(cycle)) items.push({ type: 'cycle-divider', cycle }, { type: 'cycle-status', cycle });
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
  return <div className="cycle-divider"><span>第 {cycle.cycleNumber} 轮 · Cycle {cycle.cycleNumber}</span></div>;
}

function CycleStatus({ cycle }: { cycle: RelayCodexCycle }) {
  return <p className={`inline-system ${cycle.status === 'FAILED' ? 'failed' : ''}`}>{cycle.status === 'CODEX_COMPLETED' ? '✓ Codex 已完成本轮任务' : codexCycleStatusLabel(cycle.status)}{cycle.errorText ? `：${cycle.errorText}` : ''}</p>;
}

function outboundDeliveryLabel(deliveryState: string): string {
  switch (deliveryState) {
    case 'QUEUED': return 'Codex 结果等待回传 ChatGPT';
    case 'SENT': return 'Codex 结果正在回传 ChatGPT';
    case 'DELIVERED': return 'Codex 结果已回传 ChatGPT';
    case 'UNKNOWN': return 'Codex 结果回传状态不确定';
    case 'FAILED': return 'Codex 结果未回传 ChatGPT';
    default: return `Codex 结果回传状态：${deliveryState}`;
  }
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
        if (item.type === 'cycle-divider') return <section className="automation-step" key={`divider-${item.cycle.id}`}><CycleDivider cycle={item.cycle} /></section>;
        if (item.type === 'cycle-status') return <CycleStatus key={`status-${item.cycle.id}`} cycle={item.cycle} />;
        if (item.type === 'forwarded-codex-result') return <p className="inline-system" key={item.message.id}>{outboundDeliveryLabel(item.message.deliveryState)}</p>;
        const { message } = item;
        return <section className="automation-step" key={message.id}>
          <article className={`timeline-message ${message.direction.toLowerCase()}`}>
            <header><span className="message-role">{messageTitle(message)}</span><span>{message.kind === 'MANUAL' ? '聊天' : message.kind === 'AUTOMATION' ? '自动化' : '系统'} · {message.deliveryState}</span></header>
            <pre>{message.text}</pre>
            {message.direction === 'TO_CHATGPT' && message.deliveryState === 'UNKNOWN' ? <p className="message-warning">送达结果不确定，等待你的明确处理。</p> : null}
          </article>
        </section>;
      })}
      {props.session.phase === 'WAITING_FOR_ACCEPTANCE' ? <p className="inline-system">等待人工验收</p> : null}
      {props.session.phase === 'BLOCKED' ? <p className="inline-system attention-event">⚠ 自动流程已暂停，需要人工处理</p> : null}
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
