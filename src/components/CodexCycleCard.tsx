import { codexCycleStatusLabel, type RelayCodexCycle } from '../relay-observability';

interface CodexCycleCardProps {
  cycle: RelayCodexCycle;
}

function displayIdentifier(value: string | null | undefined): string {
  return value ? value : '尚未获得';
}

export function CodexCycleCard({ cycle }: CodexCycleCardProps) {
  return <article className={`codex-cycle-card codex-cycle-${cycle.status.toLowerCase()}`}>
    <header>
      <h4>Cycle {cycle.cycleNumber} · {codexCycleStatusLabel(cycle.status)}</h4>
    </header>
    <p>Codex thread：{displayIdentifier(cycle.codexThreadId)}</p>
    <p>Codex turn：{displayIdentifier(cycle.codexTurnId)}</p>
    <section>
      <h5>Prompt 原文</h5>
      <pre>{cycle.promptText}</pre>
    </section>
    {cycle.resultText ? <section>
      <h5>Codex final text</h5>
      <pre>{cycle.resultText}</pre>
    </section> : null}
    {cycle.outboundChatgptMessageId ? <p>Outbound ChatGPT message：{cycle.outboundChatgptMessageId}</p> : null}
    {cycle.blockReason ? <p>阻塞原因：{cycle.blockReason}</p> : null}
    {cycle.errorText ? <p>错误：{cycle.errorText}</p> : null}
  </article>;
}
