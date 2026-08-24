import { FormEvent, useState } from 'react';
import { RelayCodexThreadCandidateList } from './RelayCodexThreadCandidateList';
import type { RelayCodexThreadCandidate, RelayCodexThreadTarget } from '../relay-thread-resume';

interface Draft { name: string; workingDirectory: string; maxCycles: string; maxRuntimeMinutes: string; retryTemplate: string; }

interface NewSessionFormProps {
  draft: Draft;
  target: RelayCodexThreadTarget;
  candidates: RelayCodexThreadCandidate[] | null;
  busy: boolean;
  refreshingThreads: boolean;
  onDraftChange: (draft: Draft) => void;
  onTargetChange: (target: RelayCodexThreadTarget) => void;
  onRefreshThreads: () => void;
  onSubmit: (event: FormEvent) => void;
}

export function NewSessionForm(props: NewSessionFormProps) {
  const [advanced, setAdvanced] = useState(false);
  const { draft, target } = props;
  return <section className="new-session-page"><div className="new-session-heading"><p className="eyebrow">新建 Session</p><h2>开始一段新会话</h2><p>创建本身不会发送 ChatGPT 消息，也不会启动 Codex。</p></div><form className="new-session-form" onSubmit={props.onSubmit}>
    <label>会话名称<input value={draft.name} onChange={(event) => props.onDraftChange({ ...draft, name: event.target.value })} placeholder="例如：Bridge UI 重构" /></label>
    <label>Codex 工作目录<input value={draft.workingDirectory} onChange={(event) => props.onDraftChange({ ...draft, workingDirectory: event.target.value })} placeholder="G:\\projects\\your-project" /></label>
    <fieldset className="thread-choice"><legend>Codex 对话</legend><label><input type="radio" name="thread-target" checked={target.mode === 'NEW'} onChange={() => props.onTargetChange({ mode: 'NEW' })} />新建 Codex 对话</label><label><input type="radio" name="thread-target" checked={target.mode === 'EXISTING'} onChange={() => props.onTargetChange({ mode: 'EXISTING', threadId: '' })} />继续现有 Codex 对话</label></fieldset>
    {target.mode === 'EXISTING' ? <section className="thread-picker"><button type="button" disabled={props.busy || props.refreshingThreads || !draft.workingDirectory.trim()} onClick={props.onRefreshThreads}>{props.refreshingThreads ? '正在刷新…' : '刷新对话'}</button>{props.candidates === null ? <p>请先刷新此工作目录下的可继续对话。</p> : <RelayCodexThreadCandidateList candidates={props.candidates} selectedThreadId={target.threadId} busy={props.busy} onSelect={(threadId) => props.onTargetChange({ mode: 'EXISTING', threadId })} />}</section> : null}
    <button className="advanced-toggle" type="button" onClick={() => setAdvanced(!advanced)}>高级设置 <span>{advanced ? '⌃' : '⌄'}</span></button>
    {advanced ? <div className="advanced-settings"><label>最大自动循环次数<input inputMode="numeric" value={draft.maxCycles} onChange={(event) => props.onDraftChange({ ...draft, maxCycles: event.target.value })} /></label><label>最长运行时间（分钟）<input inputMode="numeric" value={draft.maxRuntimeMinutes} onChange={(event) => props.onDraftChange({ ...draft, maxRuntimeMinutes: event.target.value })} /></label><label>Retry template<textarea rows={3} value={draft.retryTemplate} onChange={(event) => props.onDraftChange({ ...draft, retryTemplate: event.target.value })} /></label></div> : null}
    <button className="primary" type="submit" disabled={props.busy || props.refreshingThreads || (target.mode === 'EXISTING' && (!props.candidates || !target.threadId))}>创建会话</button>
  </form></section>;
}
