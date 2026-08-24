import { useState } from 'react';
import type { RelayChannelSnapshot } from '../relay-observability';
import { phaseLabel, shortId, type BridgeStatus, type PairingInfo, type RelayModule } from '../relay-ui';

interface TopBarProps {
  selected: RelayModule | null;
  snapshot: RelayChannelSnapshot | null;
  pairing: PairingInfo | null;
  bridge: BridgeStatus | null;
  canTerminate: boolean;
  stopping: boolean;
  onConnectionDetails: () => void;
  onOpenHumanIntervention: () => void;
  onTerminate: () => void;
}

export function TopBar({ selected, snapshot, pairing, bridge, canTerminate, stopping, onConnectionDetails, onOpenHumanIntervention, onTerminate }: TopBarProps) {
  const [details, setDetails] = useState<'chatgpt' | 'codex' | null>(null);
  const chatgpt = snapshot?.chatgpt;
  const codex = snapshot?.codex;
  const chatgptState = !pairing?.paired
    ? { className: 'offline', label: '● ChatGPT' }
    : chatgpt?.status === 'RECOVERY_BLOCKED'
      ? { className: 'running warning', label: '● ChatGPT 待恢复' }
      : chatgpt?.status === 'IN_FLIGHT'
        ? { className: 'running', label: '● ChatGPT 处理中' }
        : { className: 'healthy', label: '● ChatGPT' };
  const sessionStatusClass = selected?.phase === 'WAITING_FOR_ACCEPTANCE' || selected?.phase === 'BLOCKED'
    ? 'session-status channel-status running warning'
    : selected?.phase === 'FAILED'
      ? 'session-status channel-status offline'
      : 'session-status';
  return <header className="topbar">
    <div className="topbar-title"><span className="eyebrow">当前会话</span><strong>{selected?.name ?? '新建会话'}</strong></div>
    <div className="channel-cluster">
      <div className="channel-status-wrap">
        <button className={`channel-status ${chatgptState.className}`} type="button" onClick={() => setDetails(details === 'chatgpt' ? null : 'chatgpt')} title="查看 ChatGPT 通道详情">{chatgptState.label}</button>
        {details === 'chatgpt' ? <div className="channel-popover"><strong>ChatGPT 通道</strong><p>{bridge?.detail ?? (pairing?.paired ? '已配对' : '未配对')}</p><p>当前占用模块：{chatgpt?.activeModuleName ?? '—'}</p><p>当前消息：{shortId(chatgpt?.activeMessageId)}</p><p>类型：{chatgpt?.activeKind ?? '—'} · 阶段：{chatgpt?.activePhase ?? '—'}</p><p>UNKNOWN：{chatgpt?.recoveryBlockerCount ?? 0}</p><button type="button" onClick={onConnectionDetails}>连接详情</button></div> : null}
      </div>
      <div className="channel-status-wrap">
        <button className={`channel-status ${codex?.status === 'RUNNING' ? 'running' : 'healthy'}`} type="button" onClick={() => setDetails(details === 'codex' ? null : 'codex')} title="查看 Codex 通道详情">● Codex {codex?.status === 'RUNNING' ? '运行中' : '就绪'}</button>
        {details === 'codex' ? <div className="channel-popover codex-popover"><strong>Codex 通道</strong><p>Cycle：{codex?.cycleNumber ?? '—'}</p><p>Thread：{shortId(codex?.codexThreadId)}</p><p>Turn：{shortId(codex?.codexTurnId)}</p><p>当前状态：{codex?.cycleStatus ?? '空闲'}</p></div> : null}
      </div>
      {selected ? <button className={sessionStatusClass} type="button" onClick={onOpenHumanIntervention} disabled={!['WAITING_FOR_ACCEPTANCE', 'BLOCKED'].includes(selected.phase)} title={selected.phase === 'WAITING_FOR_ACCEPTANCE' ? '打开人工验收' : selected.phase === 'BLOCKED' ? '打开人工介入' : undefined}>{phaseLabel(selected.phase)}</button> : null}
      {canTerminate ? <button className="stop-button" type="button" onClick={onTerminate} disabled={stopping} title="终止当前会话">■</button> : null}
    </div>
  </header>;
}
