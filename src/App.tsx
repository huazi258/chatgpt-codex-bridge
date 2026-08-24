import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { FormEvent, useEffect, useMemo, useRef, useState } from 'react';
import { Conversation } from './components/Conversation';
import { HumanInterventionDrawer } from './components/HumanInterventionDrawer';
import { Modal } from './components/Modal';
import { NewSessionForm } from './components/NewSessionForm';
import { RelayCodexRecoveryPanel } from './components/RelayCodexRecoveryPanel';
import { SessionInspector } from './components/SessionInspector';
import { SessionSidebar } from './components/SessionSidebar';
import { TopBar } from './components/TopBar';
import type { RelayChannelSnapshot, RelayCodexCycle } from './relay-observability';
import type { RelayCodexRecoveryAction, RelayCodexThreadCandidate, RelayCodexThreadStateSnapshot, RelayCodexThreadTarget, RelayModuleCreationInput } from './relay-thread-resume';
import { terminalPhases, type BridgeStatus, type PairingInfo, type RelayKind, type RelayMessage, type RelayModule, type RelayRecoveryMessage } from './relay-ui';
import { AttentionTransitionTracker, DesktopAttentionNotifier, attentionNotice } from './desktop-attention';

const defaultRetry = '请根据既定格式，在回复最后且仅输出一个有效控制块：@@@CODEX_PROMPT@@@、@@@MODULE_DONE@@@ 或 @@@BLOCKED@@@。正在等待 Codex 用户输入时可使用 @@@CODEX_INPUT@@@。';
const emptyDraft = { name: '', workingDirectory: '', maxCycles: '12', maxRuntimeMinutes: '240', retryTemplate: defaultRetry };

export default function App() {
  const [modules, setModules] = useState<RelayModule[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [messages, setMessages] = useState<RelayMessage[]>([]);
  const [recoveryMessages, setRecoveryMessages] = useState<RelayRecoveryMessage[]>([]);
  const [cycles, setCycles] = useState<RelayCodexCycle[]>([]);
  const [snapshot, setSnapshot] = useState<RelayChannelSnapshot | null>(null);
  const [pairing, setPairing] = useState<PairingInfo | null>(null);
  const [bridge, setBridge] = useState<BridgeStatus | null>(null);
  const [notice, setNotice] = useState('正在加载本地传话状态…');
  const [busy, setBusy] = useState(false);
  const [draft, setDraft] = useState(emptyDraft);
  const [target, setTarget] = useState<RelayCodexThreadTarget>({ mode: 'NEW' });
  const [candidates, setCandidates] = useState<RelayCodexThreadCandidate[] | null>(null);
  const [refreshingThreads, setRefreshingThreads] = useState(false);
  const [recoveryState, setRecoveryState] = useState<RelayCodexThreadStateSnapshot | null>(null);
  const [text, setText] = useState('');
  const [kind, setKind] = useState<RelayKind>('MANUAL');
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => localStorage.getItem('relay.sidebar.collapsed') === 'true');
  const [inspectorCollapsed, setInspectorCollapsed] = useState(() => localStorage.getItem('relay.inspector.collapsed') === 'true');
  const [connectionOpen, setConnectionOpen] = useState(false);
  const [acceptanceOpen, setAcceptanceOpen] = useState(false);
  const [acceptanceFeedback, setAcceptanceFeedback] = useState(false);
  const [feedback, setFeedback] = useState('');
  const [blockedOpen, setBlockedOpen] = useState(false);
  const [blockedFeedback, setBlockedFeedback] = useState('');
  const [blockedReason, setBlockedReason] = useState<string | null>(null);
  const [stopConfirm, setStopConfirm] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<RelayModule | null>(null);
  const shownAcceptance = useRef<string | null>(null);
  const shownBlocked = useRef<string | null>(null);
  const attentionTracker = useRef(new AttentionTransitionTracker());
  const desktopAttention = useRef(new DesktopAttentionNotifier());
  const selected = useMemo(() => creating ? null : modules.find((module) => module.id === selectedId) ?? null, [creating, modules, selectedId]);

  async function refreshModules(preferredId?: string, preserveCreation = creating) {
    const next = await invoke<RelayModule[]>('list_relay_modules');
    setModules(next);
    if (preferredId) setSelectedId((next.find((module) => module.id === preferredId) ?? next[0] ?? null)?.id ?? null);
    else if (!preserveCreation) setSelectedId((next.find((module) => module.id === selectedId) ?? next[0] ?? null)?.id ?? null);
  }
  async function refreshMessages(moduleId = selectedId) { setMessages(moduleId ? await invoke<RelayMessage[]>('list_relay_messages', { moduleId }) : []); }
  async function refreshCycles(moduleId = selectedId) { setCycles(moduleId ? await invoke<RelayCodexCycle[]>('list_relay_codex_cycles', { moduleId }) : []); }
  async function refreshRecoveryMessages() { const next = await invoke<RelayRecoveryMessage[]>('list_relay_recovery_messages'); setRecoveryMessages(next); return next; }
  async function refreshSnapshot() { setSnapshot(await invoke<RelayChannelSnapshot>('get_relay_channel_snapshot')); }
  async function refreshPairing() { setPairing(await invoke<PairingInfo>('get_chatgpt_pairing')); }
  async function refreshRecoveryState(moduleId = selectedId) { setRecoveryState(moduleId ? await invoke<RelayCodexThreadStateSnapshot>('get_relay_codex_thread_state', { moduleId }) : null); }
  async function refreshBlockedReason(moduleId = selectedId) { setBlockedReason(moduleId ? await invoke<string | null>('get_relay_blocked_reason', { moduleId }) : null); }
  async function refreshSelected(moduleId = selectedId) { await Promise.all([refreshModules(undefined, false), refreshMessages(moduleId), refreshCycles(moduleId), refreshRecoveryMessages(), refreshSnapshot()]); }

  useEffect(() => { void Promise.all([refreshModules(), refreshPairing(), refreshRecoveryMessages(), refreshSnapshot()]).then(() => setNotice('本地传话状态已就绪。')).catch((error) => setNotice(`初始化失败：${String(error)}`)); }, []);
  useEffect(() => { void refreshMessages().catch((error) => setNotice(`无法读取消息历史：${String(error)}`)); void refreshCycles().catch((error) => setNotice(`无法读取 Codex 状态：${String(error)}`)); }, [selectedId]);
  useEffect(() => { if (selected?.phase === 'RECOVERY_REQUIRED') void refreshRecoveryState(selected.id).catch((error) => setNotice(`无法读取 Codex 恢复状态：${String(error)}`)); else setRecoveryState(null); }, [selected?.id, selected?.phase]);
  useEffect(() => { if (selected?.phase === 'BLOCKED') void refreshBlockedReason(selected.id).catch((error) => setNotice(`无法读取人工介入原因：${String(error)}`)); else setBlockedReason(null); }, [selected?.id, selected?.phase]);
  useEffect(() => {
    if (selected?.phase === 'WAITING_FOR_ACCEPTANCE' && shownAcceptance.current !== selected.id) { shownAcceptance.current = selected.id; setAcceptanceOpen(true); setAcceptanceFeedback(false); }
    if (selected?.phase !== 'WAITING_FOR_ACCEPTANCE') shownAcceptance.current = null;
  }, [selected?.id, selected?.phase]);
  useEffect(() => {
    if (selected?.phase === 'BLOCKED' && shownBlocked.current !== selected.id) { shownBlocked.current = selected.id; setBlockedOpen(true); setBlockedFeedback(''); }
    if (selected?.phase !== 'BLOCKED') shownBlocked.current = null;
  }, [selected?.id, selected?.phase]);
  useEffect(() => { localStorage.setItem('relay.sidebar.collapsed', String(sidebarCollapsed)); }, [sidebarCollapsed]);
  useEffect(() => { localStorage.setItem('relay.inspector.collapsed', String(inspectorCollapsed)); }, [inspectorCollapsed]);
  useEffect(() => {
    for (const session of attentionTracker.current.entered(modules)) {
      const detail = session.id === selected?.id
        ? session.phase === 'BLOCKED' ? blockedReason : session.phase === 'RECOVERY_REQUIRED' ? recoveryState?.summary : undefined
        : undefined;
      void desktopAttention.current.notify(attentionNotice(session, detail));
    }
  }, [modules, selected?.id, blockedReason, recoveryState?.summary]);
  useEffect(() => {
    const clear = () => void desktopAttention.current.clearAttention();
    window.addEventListener('focus', clear);
    return () => window.removeEventListener('focus', clear);
  }, []);
  useEffect(() => {
    let stopStatus: (() => void) | undefined; let stopControl: (() => void) | undefined; let stopCodex: (() => void) | undefined;
    void listen<BridgeStatus>('chatgpt-status', (event) => { setBridge(event.payload); void refreshPairing(); void refreshSelected(); }).then((unsubscribe) => { stopStatus = unsubscribe; });
    void listen<{ type: string; reason?: string }>('relay-control', (event) => { setNotice(`已处理控制回复：${event.payload.type}${event.payload.reason ? `：${event.payload.reason}` : ''}`); void refreshSelected(); }).then((unsubscribe) => { stopControl = unsubscribe; });
    void listen<{ moduleId: string }>('relay-codex', () => { void refreshSelected(); }).then((unsubscribe) => { stopCodex = unsubscribe; });
    return () => { stopStatus?.(); stopControl?.(); stopCodex?.(); };
  }, [selectedId]);

  async function refreshThreads() {
    if (!draft.workingDirectory.trim()) return setNotice('请先填写 Codex 工作目录。');
    setRefreshingThreads(true); setCandidates(null); setTarget({ mode: 'EXISTING', threadId: '' });
    try { setCandidates(await invoke<RelayCodexThreadCandidate[]>('list_relay_codex_threads_for_cwd', { workingDirectory: draft.workingDirectory.trim() })); setNotice('已刷新可继续的 Codex 对话。'); }
    catch (error) { setNotice(`刷新 Codex 对话失败：${String(error)}`); } finally { setRefreshingThreads(false); }
  }
  function updateDraft(next: typeof draft) { if (next.workingDirectory !== draft.workingDirectory) { setCandidates(null); if (target.mode === 'EXISTING') setTarget({ mode: 'EXISTING', threadId: '' }); } setDraft(next); }
  async function createSession(event: FormEvent) {
    event.preventDefault();
    if (!draft.name.trim() || !draft.workingDirectory.trim()) return setNotice('请填写会话名称和 Codex 工作目录。');
    if (target.mode === 'EXISTING' && !target.threadId) return setNotice('请刷新并选择要继续的 Codex 对话。');
    setBusy(true);
    try {
      const input: RelayModuleCreationInput = { name: draft.name.trim(), workingDirectory: draft.workingDirectory.trim(), maxCycles: Number(draft.maxCycles), maxRuntimeMinutes: Number(draft.maxRuntimeMinutes), retryTemplate: draft.retryTemplate.trim(), codexThreadTarget: target };
      const created = await invoke<RelayModule>('create_relay_module', { input });
      await refreshModules(created.id, false); setCreating(false); setDraft(emptyDraft); setTarget({ mode: 'NEW' }); setCandidates(null);
      setNotice(`已创建“${created.name}”；会话仍在等待第一条消息。`);
    } catch (error) { setNotice(`创建失败：${String(error)}`); } finally { setBusy(false); }
  }
  async function send(event: FormEvent) {
    event.preventDefault();
    if (!selected || !text.trim()) return setNotice('请输入要发送给 ChatGPT 的内容。');
    if (!pairing?.paired) { setConnectionOpen(true); return setNotice('请先在 Chrome 扩展中绑定 ChatGPT 标签页。'); }
    setBusy(true);
    try { await invoke('queue_relay_message', { moduleId: selected.id, kind, text: text.trim() }); setText(''); const blockers = await refreshRecoveryMessages(); setNotice(blockers.length ? '消息已安全入队；请先处理所有不确定送达。' : kind === 'MANUAL' ? '聊天消息已入队，不会解析控制块。' : '自动化请求已入队，其回复会按控制块处理。'); await Promise.all([refreshMessages(selected.id), refreshCycles(selected.id), refreshSnapshot()]); }
    catch (error) { setNotice(`发送失败：${String(error)}`); } finally { setBusy(false); }
  }
  async function retryUnknown(messageId: string) { setBusy(true); try { await invoke('retry_unknown_relay_message', { messageId }); setNotice('已按你的明确指令重发消息。'); await refreshSelected(); } catch (error) { setNotice(`无法重发不确定消息：${String(error)}`); } finally { setBusy(false); } }
  async function continueUnknown(messageId: string) { setBusy(true); try { await invoke('continue_unknown_relay_message_without_resend', { messageId }); setNotice('已确认不重发该消息。'); await refreshSelected(); } catch (error) { setNotice(`无法解除不确定消息阻塞：${String(error)}`); } finally { setBusy(false); } }
  async function recover(action: RelayCodexRecoveryAction) { if (!selected) return; setBusy(true); try { await invoke('recover_relay_codex', { moduleId: selected.id, action }); setNotice('已提交 Codex 对话恢复操作。'); } catch (error) { setNotice(`Codex 对话恢复失败：${String(error)}`); throw error; } finally { await refreshSelected(selected.id).catch(() => undefined); setBusy(false); } }
  async function accept() { if (!selected) return; setBusy(true); try { await invoke('accept_relay_module', { moduleId: selected.id }); setAcceptanceOpen(false); setNotice('会话已验收完成。'); } catch (error) { setNotice(`验收会话失败：${String(error)}`); } finally { await refreshSelected(selected.id).catch(() => undefined); setBusy(false); } }
  async function submitFeedback() { if (!selected || !feedback.trim()) return; setBusy(true); try { await invoke('submit_relay_acceptance_feedback', { moduleId: selected.id, text: feedback.trim() }); setFeedback(''); setAcceptanceOpen(false); setNotice('验收反馈已进入 ChatGPT 自动化队列。'); } catch (error) { setNotice(`提交验收反馈失败：${String(error)}`); } finally { await refreshSelected(selected.id).catch(() => undefined); setBusy(false); } }
  async function submitBlockedFeedback() { if (!selected || !blockedFeedback.trim()) return; setBusy(true); try { await invoke('submit_relay_blocked_feedback', { moduleId: selected.id, text: blockedFeedback.trim() }); setBlockedFeedback(''); setBlockedOpen(false); setNotice('人工回复已进入 ChatGPT 自动化队列。'); } catch (error) { setNotice(`提交人工回复失败：${String(error)}`); } finally { await refreshSelected(selected.id).catch(() => undefined); setBusy(false); } }
  async function terminate() { if (!selected) return; setBusy(true); try { await invoke('terminate_relay_module', { moduleId: selected.id }); setStopConfirm(false); setNotice('已请求终止会话。'); } catch (error) { setNotice(`终止会话失败：${String(error)}`); } finally { await refreshSelected(selected.id).catch(() => undefined); setBusy(false); } }
  async function deleteSession() { if (!deleteTarget) return; setBusy(true); try { await invoke('delete_relay_module', { moduleId: deleteTarget.id }); const remaining = modules.filter((module) => module.id !== deleteTarget.id); setDeleteTarget(null); setCreating(false); await refreshModules(remaining[0]?.id, false); setNotice('会话已删除；工作目录中的项目文件未被修改。'); } catch (error) { setNotice(`删除会话失败：${String(error)}`); } finally { setBusy(false); } }
  async function openDirectory(session: RelayModule) { try { await invoke('open_relay_working_directory', { moduleId: session.id }); } catch (error) { setNotice(`无法打开工作目录：${String(error)}`); } }

  const canTerminate = Boolean(selected && !terminalPhases.has(selected.phase) && !selected.stopAfterTurn);
  const intervention = selected?.phase === 'BLOCKED' && blockedOpen ? 'blocked' : selected?.phase === 'WAITING_FOR_ACCEPTANCE' && acceptanceOpen ? 'acceptance' : null;
  const selectedRecoveryMessages = selected ? recoveryMessages.filter((message) => message.moduleId === selected.id) : [];
  return <main className="relay-app">
    <SessionSidebar sessions={modules} selectedId={selectedId} creating={creating} collapsed={sidebarCollapsed} busy={busy} onCollapsedChange={setSidebarCollapsed} onCreate={() => { setSelectedId(null); setCreating(true); setNotice('请填写新会话的基本信息。'); }} onSelect={(id) => { setCreating(false); setSelectedId(id); }} onOpenDirectory={(session) => void openDirectory(session)} onDelete={setDeleteTarget} />
    <div className="relay-main"><TopBar selected={selected} snapshot={snapshot} pairing={pairing} bridge={bridge} canTerminate={canTerminate} stopping={busy} onConnectionDetails={() => setConnectionOpen(true)} onOpenHumanIntervention={() => { if (selected?.phase === 'WAITING_FOR_ACCEPTANCE') setAcceptanceOpen(true); if (selected?.phase === 'BLOCKED') setBlockedOpen(true); }} onTerminate={() => setStopConfirm(true)} />
      <div className="main-content">{creating || !selected ? <NewSessionForm draft={draft} target={target} candidates={candidates} busy={busy} refreshingThreads={refreshingThreads} onDraftChange={updateDraft} onTargetChange={(next) => { setTarget(next); setCandidates(next.mode === 'EXISTING' ? candidates : null); }} onRefreshThreads={() => void refreshThreads()} onSubmit={createSession} /> : <>
        {!pairing?.paired ? <button className="pairing-nudge" type="button" onClick={() => setConnectionOpen(true)}>ChatGPT 未配对 · 打开连接详情</button> : null}
        {selected.phase === 'RECOVERY_REQUIRED' ? <RelayCodexRecoveryPanel snapshot={recoveryState} busy={busy} onAction={recover} /> : null}
        <Conversation session={selected} messages={messages} cycles={cycles} recoveryMessages={recoveryMessages} notice={notice} text={text} kind={kind} busy={busy} onTextChange={setText} onKindChange={setKind} onSend={send} onRetryUnknown={(id) => void retryUnknown(id)} onContinueUnknown={(id) => void continueUnknown(id)} />
      </>}</div>
    </div>
    {selected && !creating ? intervention ? <HumanInterventionDrawer kind={intervention} collapsed={inspectorCollapsed} busy={busy} blockedReason={blockedReason} feedback={intervention === 'blocked' ? blockedFeedback : feedback} recoveryMessages={selectedRecoveryMessages} acceptanceFeedbackOpen={acceptanceFeedback} onCollapsedChange={setInspectorCollapsed} onClose={() => intervention === 'blocked' ? setBlockedOpen(false) : setAcceptanceOpen(false)} onFeedbackChange={intervention === 'blocked' ? setBlockedFeedback : setFeedback} onSubmitBlocked={() => void submitBlockedFeedback()} onAccept={() => void accept()} onStartAcceptanceFeedback={() => setAcceptanceFeedback(true)} onBackToAcceptance={() => setAcceptanceFeedback(false)} onSubmitAcceptanceFeedback={() => void submitFeedback()} /> : <SessionInspector session={selected} cycles={cycles} snapshot={snapshot} collapsed={inspectorCollapsed} onCollapsedChange={setInspectorCollapsed} onOpenDirectory={() => void openDirectory(selected)} /> : null}
    {connectionOpen ? <Modal title="ChatGPT 连接详情" onClose={() => setConnectionOpen(false)}><p>{bridge?.detail ?? '在 Chrome 扩展中选择当前已登录的 ChatGPT 对话后配对。'}</p><label>本机地址<input readOnly value={pairing?.endpoint ?? '正在启动…'} /></label><label>一次性配对密钥<input readOnly value={pairing?.pairingSecret ?? '正在生成…'} /></label><button type="button" onClick={() => void refreshPairing().catch((error) => setNotice(String(error)))}>刷新连接状态</button></Modal> : null}
    {stopConfirm && selected ? <Modal title="终止当前会话？" onClose={() => setStopConfirm(false)}><p>这将停止自动循环，但会保留已有的会话和执行记录。</p>{selected.phase === 'CODEX_RUNNING' ? <p>当前 Codex 回合会自然结束；其结果不会回传 ChatGPT。</p> : null}<div className="modal-actions"><button type="button" onClick={() => setStopConfirm(false)}>取消</button><button className="danger" type="button" disabled={busy} onClick={() => void terminate()}>终止会话</button></div></Modal> : null}
    {deleteTarget ? <Modal title={`删除“${deleteTarget.name}”？`} onClose={() => setDeleteTarget(null)}><p>此操作会删除该会话的消息历史和运行状态，但不会删除工作目录中的任何项目文件。</p><div className="modal-actions"><button type="button" onClick={() => setDeleteTarget(null)}>取消</button><button className="danger" type="button" disabled={busy} onClick={() => void deleteSession()}>删除会话</button></div></Modal> : null}
  </main>;
}
