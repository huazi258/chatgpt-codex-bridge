import type { RelayChannelSnapshot, RelayCodexCycle } from '../relay-observability';
import { currentCycle, phaseLabel, shortId, type RelayModule } from '../relay-ui';

interface SessionInspectorProps {
  session: RelayModule;
  cycles: RelayCodexCycle[];
  snapshot: RelayChannelSnapshot | null;
  collapsed: boolean;
  onCollapsedChange: (collapsed: boolean) => void;
  onOpenDirectory: () => void;
}

function CopyId({ value }: { value?: string | null }) {
  return <span className="copyable-id" title={value ?? undefined}>{shortId(value)}{value ? <button type="button" aria-label="复制 ID" onClick={() => void navigator.clipboard?.writeText(value)}>复制</button> : null}</span>;
}

function runtimeUsage(session: RelayModule): string {
  if (!session.moduleStartedAt) return `尚未开始 / ${session.maxRuntimeMinutes} 分钟`;
  const startedAt = Date.parse(session.moduleStartedAt);
  if (Number.isNaN(startedAt)) return `尚未开始 / ${session.maxRuntimeMinutes} 分钟`;
  const elapsedMinutes = Math.max(0, Math.floor((Date.now() - startedAt) / 60_000));
  return `${elapsedMinutes} 分钟 / ${session.maxRuntimeMinutes} 分钟`;
}

export function SessionInspector({ session, cycles, snapshot, collapsed, onCollapsedChange, onOpenDirectory }: SessionInspectorProps) {
  const cycle = currentCycle(cycles);
  return <aside className={`session-inspector ${collapsed ? 'collapsed' : ''}`}>
    <div className="inspector-heading"><div>{!collapsed ? <><strong>会话信息</strong><small>Session Inspector</small></> : null}</div><button className="icon-button" type="button" aria-label={collapsed ? '展开会话信息' : '收起会话信息'} onClick={() => onCollapsedChange(!collapsed)}>{collapsed ? '‹' : '›'}</button></div>
    {!collapsed ? <div className="inspector-content">
      <section><h3>状态</h3><dl><dt>当前状态</dt><dd>{phaseLabel(session.phase)}</dd><dt>Cycle</dt><dd>{session.startedCycles} / {session.maxCycles}</dd><dt>Runtime</dt><dd>{runtimeUsage(session)}</dd><dt>Invalid replies</dt><dd>{session.invalidReplyCount}</dd></dl></section>
      <section><h3>ChatGPT</h3><dl><dt>当前消息</dt><dd><CopyId value={snapshot?.chatgpt.activeMessageId} /></dd><dt>当前阶段</dt><dd>{snapshot?.chatgpt.activePhase ?? '—'}</dd></dl></section>
      <section><h3>Codex</h3><dl><dt>Thread</dt><dd><CopyId value={cycle?.codexThreadId ?? session.codexThreadId ?? session.resumeThreadId} /></dd><dt>Turn</dt><dd><CopyId value={cycle?.codexTurnId ?? snapshot?.codex.codexTurnId} /></dd></dl></section>
      <section><h3>工作目录</h3><p className="working-directory" title={session.workingDirectory}>{session.workingDirectory}</p><button type="button" onClick={onOpenDirectory}>打开文件夹</button></section>
      <section><h3>运行限制</h3><p>最大循环次数：{session.maxCycles}</p><p>最长运行时间：{session.maxRuntimeMinutes} 分钟</p></section>
    </div> : null}
  </aside>;
}
