import type { RelayChannelSnapshot } from '../relay-observability';
import { codexCycleStatusLabel } from '../relay-observability';

interface GlobalChannelStatusProps {
  snapshot: RelayChannelSnapshot | null;
}

function chatgptChannelStatusLabel(status: RelayChannelSnapshot['chatgpt']['status']): string {
  switch (status) {
    case 'IDLE': return '空闲';
    case 'IN_FLIGHT': return '忙碌';
    case 'RECOVERY_BLOCKED': return '恢复阻塞';
  }
}

function codexChannelStatusLabel(status: RelayChannelSnapshot['codex']['status']): string {
  switch (status) {
    case 'IDLE': return '空闲';
    case 'RUNNING': return '运行中';
  }
}

function displayValue(value: string | number | null | undefined): string {
  return value === null || value === undefined || value === '' ? '尚未获得' : String(value);
}

export function GlobalChannelStatus({ snapshot }: GlobalChannelStatusProps) {
  if (!snapshot) {
    return <section className="global-channel-status" aria-label="全局通道状态">
      <h3>全局通道状态</h3>
      <p>正在读取通道状态…</p>
    </section>;
  }

  const { chatgpt, codex } = snapshot;
  return <section className="global-channel-status" aria-label="全局通道状态">
    <h3>全局通道状态</h3>
    <div className="global-channel-status-cards">
      <article className="channel-status-card chatgpt-channel-status">
        <h4>ChatGPT 通道：{chatgptChannelStatusLabel(chatgpt.status)}</h4>
        <p>当前占用模块：{displayValue(chatgpt.activeModuleName)}</p>
        <p>当前消息：{displayValue(chatgpt.activeMessageId)}</p>
        <p>当前类型：{displayValue(chatgpt.activeKind)}</p>
        <p>当前阶段：{displayValue(chatgpt.activePhase)}</p>
        <p>待恢复 UNKNOWN：{chatgpt.recoveryBlockerCount} 条</p>
      </article>
      <article className="channel-status-card codex-channel-status">
        <h4>Codex 通道：{codexChannelStatusLabel(codex.status)}</h4>
        <p>当前模块：{displayValue(codex.activeModuleName)}</p>
        <p>Cycle：{displayValue(codex.cycleNumber)}</p>
        <p>Codex thread：{displayValue(codex.codexThreadId)}</p>
        <p>Codex turn：{displayValue(codex.codexTurnId)}</p>
        <p>当前状态：{codex.cycleStatus ? codexCycleStatusLabel(codex.cycleStatus) : '尚未获得'}</p>
      </article>
    </div>
  </section>;
}
