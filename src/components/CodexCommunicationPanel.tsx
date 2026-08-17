import { CodexCycleCard } from './CodexCycleCard';
import type { RelayCodexCycle } from '../relay-observability';

interface CodexCommunicationPanelProps {
  cycles: RelayCodexCycle[] | null;
}

export function CodexCommunicationPanel({ cycles }: CodexCommunicationPanelProps) {
  return <section className="codex-communication-panel" aria-label="Codex 通讯">
    <h3>Codex 通讯</h3>
    {cycles === null ? <p>正在读取 Codex 通讯状态…</p> : null}
    {cycles?.length === 0 ? <p>尚未开始 Codex 循环。</p> : null}
    {cycles?.map((cycle) => <CodexCycleCard cycle={cycle} key={cycle.id} />)}
  </section>;
}
