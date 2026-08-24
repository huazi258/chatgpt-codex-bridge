import type { RelayCodexThreadCandidate } from '../relay-thread-resume';

interface RelayCodexThreadCandidateListProps {
  candidates: RelayCodexThreadCandidate[];
  selectedThreadId?: string | null;
  busy?: boolean;
  onSelect: (threadId: string) => void;
}

function shortThreadId(threadId: string) {
  return threadId.length > 12 ? `${threadId.slice(0, 12)}…` : threadId;
}

function recencyLabel(recencyAt: number | null) {
  if (recencyAt === null) return '更新时间：未知';
  const date = new Date(recencyAt * 1000);
  return Number.isNaN(date.getTime()) ? '更新时间：未知' : `更新时间：${date.toLocaleString('zh-CN')}`;
}

export function RelayCodexThreadCandidateList({
  candidates,
  selectedThreadId,
  busy = false,
  onSelect,
}: RelayCodexThreadCandidateListProps) {
  if (candidates.length === 0) return <p className="execution-status">未发现可显示的 Codex 对话。</p>;
  return <div className="relay-thread-candidates" aria-label="Codex 对话候选列表">
    {candidates.map((candidate) => {
      const disabled = busy || !candidate.selectable;
      const selected = candidate.threadId === selectedThreadId;
      return <article className={`relay-thread-candidate ${selected ? 'selected' : ''}`} key={candidate.threadId}>
        <div>
          <strong>{candidate.name?.trim() || '未命名 Codex 对话'}</strong>
          <p>来源：{candidate.source} · 状态：{candidate.status}</p>
          <p>{candidate.branch ? `分支：${candidate.branch}` : '无分支信息'} · {recencyLabel(candidate.recencyAt)}</p>
          <p>对话 ID：{shortThreadId(candidate.threadId)}</p>
          {!candidate.selectable && candidate.disabledReason ? <p className="relay-thread-disabled-reason">{candidate.disabledReason}</p> : null}
        </div>
        <button
          className="secondary"
          type="button"
          disabled={disabled}
          aria-disabled={disabled}
          aria-pressed={selected}
          onClick={() => onSelect(candidate.threadId)}
        >
          {selected ? '已选择' : candidate.selectable ? '选择此对话' : '不可选择'}
        </button>
      </article>;
    })}
  </div>;
}
