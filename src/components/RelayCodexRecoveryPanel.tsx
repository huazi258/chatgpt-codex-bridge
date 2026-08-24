import { invoke } from '@tauri-apps/api/core';
import { useState } from 'react';
import type {
  RelayCodexRecoveryAction,
  RelayCodexRecoveryAllowedAction,
  RelayCodexThreadCandidate,
  RelayCodexThreadStateSnapshot,
} from '../relay-thread-resume';
import { RelayCodexThreadCandidateList } from './RelayCodexThreadCandidateList';

interface RelayCodexRecoveryPanelProps {
  snapshot: RelayCodexThreadStateSnapshot | null;
  busy: boolean;
  onAction: (action: RelayCodexRecoveryAction) => Promise<void>;
}

const labels: Record<RelayCodexRecoveryAllowedAction['type'], string> = {
  RETRY_RESUME: '重新尝试继续此对话',
  REACQUIRE_THREAD: '重新获取此对话',
  START_NEW_THREAD: '改用新 Codex 对话',
  RETRY_TURN_START: '重试发送任务',
  SELECT_EXISTING_THREAD: '选择现有 Codex 对话',
};

function RegistryDetails({ snapshot }: { snapshot: RelayCodexThreadStateSnapshot }) {
  return <div className="recovery-details">
    <p>Codex 工作目录：{snapshot.workingDirectory}</p>
    <p>预期对话：{snapshot.intendedThreadId ?? '无'}</p>
    <p>已获取对话：{snapshot.acquiredThreadId ?? '无'}</p>
    <p>登记状态：{snapshot.registry?.state ?? '无本地登记'}</p>
    {snapshot.pendingCycle ? <section className="execution-result"><strong>待处理提示 · Cycle {snapshot.pendingCycle.cycleNumber} · {snapshot.pendingCycle.status}</strong><pre>{snapshot.pendingCycle.promptText}</pre></section> : null}
  </div>;
}

export function RelayCodexRecoveryPanel({ snapshot, busy, onAction }: RelayCodexRecoveryPanelProps) {
  const [showChooser, setShowChooser] = useState(false);
  const [candidates, setCandidates] = useState<RelayCodexThreadCandidate[] | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (snapshot === null) return <section className="form-section relay-codex-recovery-panel"><h3>正在读取 Codex 对话恢复状态…</h3></section>;
  const currentSnapshot = snapshot;

  async function refreshCandidates() {
    setRefreshing(true);
    setCandidates(null);
    setError(null);
    try {
      setCandidates(await invoke<RelayCodexThreadCandidate[]>('list_relay_codex_threads_for_cwd', { workingDirectory: currentSnapshot.workingDirectory }));
    } catch (cause) {
      setError(`刷新 Codex 对话失败：${String(cause)}`);
    } finally {
      setRefreshing(false);
    }
  }

  async function apply(action: RelayCodexRecoveryAllowedAction) {
    if (action.type === 'SELECT_EXISTING_THREAD') {
      setShowChooser(true);
      return;
    }
    if (action.type === 'START_NEW_THREAD' && !window.confirm('改用新 Codex 对话是明确选择；系统不会自动匹配可能已创建的对话。是否继续？')) return;
    setError(null);
    try {
      await onAction(action as RelayCodexRecoveryAction);
    } catch (cause) {
      setError(`恢复操作失败：${String(cause)}`);
    }
  }

  async function selectExisting(threadId: string) {
    setError(null);
    try {
      await onAction({ type: 'SELECT_EXISTING_THREAD', threadId });
      setShowChooser(false);
      setCandidates(null);
    } catch (cause) {
      setError(`恢复操作失败：${String(cause)}`);
    }
  }

  return <section className="form-section relay-codex-recovery-panel">
    <h3>Codex 对话恢复</h3>
    <p className="relay-action-warning">{currentSnapshot.summary}</p>
    <RegistryDetails snapshot={currentSnapshot} />
    {error ? <p className="relay-thread-disabled-reason" role="status">{error}</p> : null}
    <div className="inline-actions">
      {currentSnapshot.allowedActions.map((action) => <button className={action.type === 'START_NEW_THREAD' ? 'danger' : 'secondary'} key={action.type} type="button" disabled={busy || refreshing} onClick={() => void apply(action)}>{labels[action.type]}</button>)}
    </div>
    {showChooser ? <section className="codex-thread-picker recovery-thread-picker">
      <h4>选择现有 Codex 对话</h4>
      <p className="execution-status">工作目录固定为：{currentSnapshot.workingDirectory}</p>
      <button className="secondary" type="button" disabled={busy || refreshing} onClick={() => void refreshCandidates()}>{refreshing ? '正在刷新…' : '刷新对话'}</button>
      {candidates === null ? <p className="execution-status">请刷新对话后再选择。</p> : <RelayCodexThreadCandidateList candidates={candidates} busy={busy || refreshing} onSelect={(threadId) => void selectExisting(threadId)} />}
    </section> : null}
  </section>;
}
