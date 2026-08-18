import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { FormEvent, useEffect, useMemo, useState } from 'react';
import { CodexCommunicationPanel } from './components/CodexCommunicationPanel';
import { GlobalChannelStatus } from './components/GlobalChannelStatus';
import { RelayAcceptancePanel } from './components/RelayAcceptancePanel';
import { RelayModuleActions } from './components/RelayModuleActions';
import { CodexHumanInputPanel } from './components/CodexHumanInputPanel';
import type { RelayChannelSnapshot, RelayCodexCycle } from './relay-observability';
import type { RelayCodexInputRequest } from './relay-codex-input';

type RelayKind = 'MANUAL' | 'AUTOMATION';

interface PairingInfo { endpoint: string; pairingSecret: string; paired: boolean; }
interface BridgeStatus { phase: string; detail: string; }
interface RelayModule {
  id: string; name: string; workingDirectory: string; maxCycles: number; maxRuntimeMinutes: number;
  retryTemplate: string; phase: string; codexThreadId?: string; invalidReplyCount: number; startedCycles: number;
  stopAfterTurn: boolean;
}
interface RelayMessage {
  id: string; sequenceNumber: number; direction: 'TO_CHATGPT' | 'FROM_CHATGPT' | 'TO_CODEX' | 'FROM_CODEX';
  kind: 'MANUAL' | 'AUTOMATION' | 'SYSTEM'; text: string; deliveryState: string;
}
interface RelayRecoveryMessage {
  messageId: string; moduleId: string; moduleName: string; sequenceNumber: number; kind: RelayKind; createdAt: string;
}

const defaultRetry = '请根据既定格式，在回复最后且仅输出一个有效控制块：@@@CODEX_PROMPT@@@、@@@MODULE_DONE@@@ 或 @@@BLOCKED@@@。';
const terminalPhases = new Set(['COMPLETED', 'STOPPED']);

export default function App() {
  const [modules, setModules] = useState<RelayModule[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [isCreatingModule, setIsCreatingModule] = useState(false);
  const [messages, setMessages] = useState<RelayMessage[]>([]);
  const [recoveryMessages, setRecoveryMessages] = useState<RelayRecoveryMessage[]>([]);
  const [codexCycles, setCodexCycles] = useState<RelayCodexCycle[]>([]);
  const [codexInputRequests, setCodexInputRequests] = useState<RelayCodexInputRequest[]>([]);
  const [channelSnapshot, setChannelSnapshot] = useState<RelayChannelSnapshot | null>(null);
  const [pairing, setPairing] = useState<PairingInfo | null>(null);
  const [bridge, setBridge] = useState<BridgeStatus | null>(null);
  const [notice, setNotice] = useState('正在加载本地传话状态…');
  const [busy, setBusy] = useState(false);
  const [draft, setDraft] = useState({ name: '', workingDirectory: '', maxCycles: '12', maxRuntimeMinutes: '240', retryTemplate: defaultRetry });
  const [text, setText] = useState('');
  const [kind, setKind] = useState<RelayKind>('MANUAL');
  const selected = useMemo(
    () => isCreatingModule ? null : modules.find((item) => item.id === selectedId) ?? null,
    [isCreatingModule, modules, selectedId],
  );

  async function refreshModules(preferredId?: string, preserveCreationView = isCreatingModule) {
    const next = await invoke<RelayModule[]>('list_relay_modules');
    setModules(next);
    if (preferredId) {
      setSelectedId((next.find((item) => item.id === preferredId) ?? next[0] ?? null)?.id ?? null);
    } else if (!preserveCreationView) {
      setSelectedId((next.find((item) => item.id === selectedId) ?? next[0] ?? null)?.id ?? null);
    }
  }
  async function refreshMessages(moduleId = selectedId) {
    if (!moduleId) return setMessages([]);
    setMessages(await invoke<RelayMessage[]>('list_relay_messages', { moduleId }));
  }
  async function refreshRecoveryMessages() {
    const next = await invoke<RelayRecoveryMessage[]>('list_relay_recovery_messages');
    setRecoveryMessages(next);
    return next;
  }
  async function refreshCodexCycles(moduleId = selectedId) {
    if (!moduleId) return setCodexCycles([]);
    setCodexCycles(await invoke<RelayCodexCycle[]>('list_relay_codex_cycles', { moduleId }));
  }
  async function refreshCodexInputRequests(moduleId = selectedId) {
    if (!moduleId) return setCodexInputRequests([]);
    try { setCodexInputRequests(await invoke<RelayCodexInputRequest[]>('list_relay_codex_input_requests', { moduleId })); }
    catch { setCodexInputRequests([]); }
  }
  async function refreshChannelSnapshot() {
    setChannelSnapshot(await invoke<RelayChannelSnapshot>('get_relay_channel_snapshot'));
  }
  async function refreshPairing() { setPairing(await invoke<PairingInfo>('get_chatgpt_pairing')); }

  useEffect(() => {
    Promise.all([refreshModules(), refreshPairing(), refreshRecoveryMessages(), refreshChannelSnapshot()])
      .then(() => setNotice('本地传话状态已就绪。先在 Chrome 扩展中绑定当前 ChatGPT 对话。'))
      .catch((error) => setNotice(`初始化失败：${String(error)}`));
  }, []);
  useEffect(() => {
    void refreshMessages().catch((error) => setNotice(`无法读取消息历史：${String(error)}`));
    void refreshCodexCycles().catch((error) => setNotice(`无法读取 Codex 通讯状态：${String(error)}`));
    void refreshCodexInputRequests().catch((error) => setNotice(`无法读取 Codex 输入请求：${String(error)}`));
  }, [selectedId]);
  useEffect(() => {
    let stopStatus: (() => void) | undefined;
    let stopControl: (() => void) | undefined;
    let stopCodex: (() => void) | undefined;
    void listen<BridgeStatus>('chatgpt-status', (event) => {
      setBridge(event.payload); void refreshPairing().catch(() => undefined); void refreshMessages().catch(() => undefined); void refreshRecoveryMessages().catch(() => undefined); void refreshCodexCycles().catch(() => undefined); void refreshChannelSnapshot().catch(() => undefined);
    }).then((unsubscribe) => { stopStatus = unsubscribe; });
    void listen<{ type: string; reason?: string }>('relay-control', (event) => {
      setNotice(`已处理 ChatGPT 控制回复：${event.payload.type}${event.payload.reason ? `：${event.payload.reason}` : ''}`);
      void refreshModules().catch(() => undefined); void refreshMessages().catch(() => undefined); void refreshRecoveryMessages().catch(() => undefined); void refreshCodexCycles().catch(() => undefined); void refreshChannelSnapshot().catch(() => undefined);
    }).then((unsubscribe) => { stopControl = unsubscribe; });
    void listen<{ moduleId: string }>('relay-codex', () => {
      void refreshCodexCycles().catch(() => undefined); void refreshCodexInputRequests().catch(() => undefined); void refreshChannelSnapshot().catch(() => undefined); void refreshModules().catch(() => undefined);
    }).then((unsubscribe) => { stopCodex = unsubscribe; });
    return () => { stopStatus?.(); stopControl?.(); stopCodex?.(); };
  }, [selectedId]);

  async function createModule(event: FormEvent) {
    event.preventDefault();
    if (!draft.name.trim() || !draft.workingDirectory.trim()) return setNotice('请填写模块名称和 Codex 工作目录。');
    setBusy(true);
    try {
      const module = await invoke<RelayModule>('create_relay_module', { input: {
        name: draft.name.trim(), workingDirectory: draft.workingDirectory.trim(), maxCycles: Number(draft.maxCycles),
        maxRuntimeMinutes: Number(draft.maxRuntimeMinutes), retryTemplate: draft.retryTemplate.trim()
      }});
      await refreshModules(module.id, false);
      setIsCreatingModule(false);
      setDraft({ name: '', workingDirectory: '', maxCycles: '12', maxRuntimeMinutes: '240', retryTemplate: defaultRetry });
      setNotice(`已创建“${module.name}”。创建不会发送消息或启动 Codex。`);
    } catch (error) { setNotice(`创建失败：${String(error)}`); } finally { setBusy(false); }
  }

  async function send(event: FormEvent) {
    event.preventDefault();
    if (!selected) return setNotice('请先创建或选择一个传话模块。');
    if (!pairing?.paired) return setNotice('请先在 Chrome 扩展中绑定 ChatGPT 标签页。');
    if (!text.trim()) return setNotice('请输入要发送给 ChatGPT 的内容。');
    setBusy(true);
    try {
      await invoke('queue_relay_message', { moduleId: selected.id, kind, text: text.trim() });
      setText('');
      const blockers = await refreshRecoveryMessages();
      setNotice(blockers.length > 0
        ? '存在待人工处理的不确定送达消息；当前消息已安全入队，待你明确处理全部不确定消息后才会发送。'
        : kind === 'MANUAL' ? '手动消息已入队；其回复只展示，不会触发 Codex。' : '自动化请求已入队；其对应回复将按控制块规则处理。');
      await Promise.all([refreshMessages(selected.id), refreshCodexCycles(selected.id), refreshChannelSnapshot()]);
    } catch (error) { setNotice(`发送失败：${String(error)}`); } finally { setBusy(false); }
  }

  async function retryUnknownMessage(messageId: string) {
    setBusy(true);
    try {
      await invoke('retry_unknown_relay_message', { messageId });
      setNotice('已按你的明确指令重新发送该不确定消息。');
      await Promise.all([refreshMessages(selectedId), refreshRecoveryMessages(), refreshModules(), refreshCodexCycles(selectedId), refreshChannelSnapshot()]);
    } catch (error) { setNotice(`无法重发不确定消息：${String(error)}`); } finally { setBusy(false); }
  }

  async function continueUnknownMessageWithoutResend(messageId: string) {
    setBusy(true);
    try {
      await invoke('continue_unknown_relay_message_without_resend', { messageId });
      setNotice('已确认不重发该不确定消息；系统会在所有不确定消息均处理后继续队列。');
      await Promise.all([refreshMessages(selectedId), refreshRecoveryMessages(), refreshModules(), refreshCodexCycles(selectedId), refreshChannelSnapshot()]);
    } catch (error) { setNotice(`无法解除不确定消息阻塞：${String(error)}`); } finally { setBusy(false); }
  }

  async function refreshAfterModuleAction(moduleId: string) {
    await Promise.all([
      refreshModules(undefined, false),
      refreshMessages(moduleId),
      refreshRecoveryMessages(),
      refreshCodexCycles(moduleId),
      refreshCodexInputRequests(moduleId),
      refreshChannelSnapshot(),
    ]);
  }

  async function submitCodexInput(inputRequestId: string, answers: { questionId: string; answer: string }[]) {
    if (!selected) return;
    setBusy(true);
    try { await invoke('submit_relay_codex_input', { inputRequestId, answers }); setNotice('答案已发送，正在等待 Codex 确认'); await Promise.all([refreshCodexInputRequests(selected.id), refreshCodexCycles(selected.id), refreshChannelSnapshot(), refreshModules()]); }
    catch (error) { setNotice(`提交 Codex 输入失败：${String(error)}`); } finally { setBusy(false); }
  }

  async function acceptSelectedModule() {
    if (!selected) return;
    setBusy(true);
    try {
      await invoke('accept_relay_module', { moduleId: selected.id });
      setNotice('模块已验收完成。');
    } catch (error) {
      const detail = String(error);
      setNotice(`验收模块失败：${detail}`);
      throw error;
    } finally {
      try {
        await refreshAfterModuleAction(selected.id);
      } catch (error) {
        setNotice(`无法刷新模块状态：${String(error)}`);
      }
      setBusy(false);
    }
  }

  async function submitAcceptanceFeedback(text: string) {
    if (!selected) return;
    setBusy(true);
    try {
      await invoke('submit_relay_acceptance_feedback', { moduleId: selected.id, text });
      setNotice('验收反馈已进入 ChatGPT 自动化队列。');
    } catch (error) {
      const detail = String(error);
      setNotice(`提交验收反馈失败：${detail}`);
      throw error;
    } finally {
      try {
        await refreshAfterModuleAction(selected.id);
      } catch (error) {
        setNotice(`无法刷新模块状态：${String(error)}`);
      }
      setBusy(false);
    }
  }

  async function terminateSelectedModule() {
    if (!selected) return;
    setBusy(true);
    try {
      await invoke('terminate_relay_module', { moduleId: selected.id });
      setNotice('已请求终止模块。');
    } catch (error) {
      const detail = String(error);
      setNotice(`终止模块失败：${detail}`);
      throw error;
    } finally {
      try {
        await refreshAfterModuleAction(selected.id);
      } catch (error) {
        setNotice(`无法刷新模块状态：${String(error)}`);
      }
      setBusy(false);
    }
  }

  return <main className="shell relay-shell">
    <aside className="sidebar">
      <div><p className="eyebrow">CONVERSATION RELAY V2</p><h1>传话模块</h1><p className="muted">一个模块对应一个 ChatGPT 对话和一个由中间件持有的 Codex 对话。</p></div>
      <p className={`bridge-state ${pairing?.paired ? 'online' : 'offline'}`}>{pairing?.paired ? 'ChatGPT 已连接' : 'ChatGPT 未连接'}</p>
      <nav aria-label="传话模块列表"><p className="section-label">模块 · {modules.length}</p><button className={`new-module-entry ${isCreatingModule || modules.length === 0 ? 'selected' : ''}`} type="button" onClick={() => { setSelectedId(null); setIsCreatingModule(true); setNotice('请填写新模块的信息。'); }} disabled={busy}>新建模块</button>{modules.length === 0 ? <p className="empty">还没有传话模块。</p> : modules.map((module) => <button className={`module-card ${!isCreatingModule && module.id === selectedId ? 'selected' : ''}`} key={module.id} onClick={() => { setIsCreatingModule(false); setSelectedId(module.id); setNotice(`已打开“${module.name}”。`); }} disabled={busy}><strong>{module.name}</strong><span>{module.phase}</span></button>)}</nav>
    </aside>
    <section className="workspace relay-workspace">
      <header><div><p className="eyebrow">受控文本传话</p><h2>{selected?.name ?? '创建传话模块'}</h2></div><span className="status-pill">{selected?.phase ?? 'READY'}</span></header>
      <p className="notice" role="status">{notice}</p>
      <section className="form-section connection-card"><div><h3>ChatGPT 浏览器连接</h3><p className="execution-status">{bridge?.detail ?? '在 Chrome 扩展中选择当前已登录的 ChatGPT 对话后配对。'}</p></div><label>本机地址<input readOnly value={pairing?.endpoint ?? '正在启动…'} /></label><label>一次性配对密钥<input readOnly value={pairing?.pairingSecret ?? '正在生成…'} /></label><button className="secondary" type="button" onClick={() => void refreshPairing().catch((error) => setNotice(String(error)))} disabled={busy}>刷新连接状态</button></section>
      {recoveryMessages.length > 0 ? <section className="form-section uncertain-delivery"><h3>待人工处理的不确定送达消息</h3><p>存在待人工处理的不确定送达消息</p><p className="execution-status">为遵守不确定送达规则，所有消息会保持安全阻塞，直到你逐条明确决定。</p>{recoveryMessages.map((message) => <article className="message system" key={message.messageId}><header><strong>{message.moduleName} · 第 {message.sequenceNumber} 条 · {message.kind === 'MANUAL' ? '手动' : '自动化'}</strong><span>UNKNOWN</span></header><div className="uncertain-delivery"><button className="secondary" disabled={busy} type="button" onClick={() => void retryUnknownMessage(message.messageId)}>明确重发这条消息</button><button className="secondary" disabled={busy} type="button" onClick={() => void continueUnknownMessageWithoutResend(message.messageId)}>不重发并继续</button></div></article>)}</section> : null}
      {!selected ? <form className="form-section" onSubmit={createModule}><h3>新建模块</h3><label>模块名称<input value={draft.name} onChange={(event) => setDraft({ ...draft, name: event.target.value })} placeholder="例如：中间件 V2 实现" /></label><label>Codex 工作目录<input value={draft.workingDirectory} onChange={(event) => setDraft({ ...draft, workingDirectory: event.target.value })} placeholder="G:\\projects\\your-project" /></label><div className="budget-grid"><label>最大自动循环次数<input inputMode="numeric" value={draft.maxCycles} onChange={(event) => setDraft({ ...draft, maxCycles: event.target.value })} /></label><label>模块最长时间（分钟）<input inputMode="numeric" value={draft.maxRuntimeMinutes} onChange={(event) => setDraft({ ...draft, maxRuntimeMinutes: event.target.value })} /></label></div><label>协议重试模板<textarea rows={3} value={draft.retryTemplate} onChange={(event) => setDraft({ ...draft, retryTemplate: event.target.value })} /></label><button className="primary" disabled={busy} type="submit">创建传话模块</button></form> : <>
        <GlobalChannelStatus snapshot={channelSnapshot} />
        <section className="form-section relay-summary"><div><h3>模块状态</h3><p className="execution-status">工作目录：{selected.workingDirectory}</p></div><p className="execution-status">已开始循环：{selected.startedCycles} / {selected.maxCycles} · 最长运行：{selected.maxRuntimeMinutes} 分钟 · 无效自动化回复：{selected.invalidReplyCount}</p>{selected.codexThreadId ? <p className="protocol-result">Codex 对话：{selected.codexThreadId}</p> : <p className="execution-status">首个有效 CODEX_PROMPT 到来后才会创建 Codex 对话。</p>}</section>
        {selected.phase === 'COMPLETED' ? <section className="form-section relay-terminal-notice"><h3>已验收完成</h3><p className="execution-status">该模块已完成验收，保留历史记录且不会再发送消息或启动 Codex。</p></section> : null}
        {selected.phase === 'STOPPED' ? <section className="form-section relay-terminal-notice"><h3>已终止</h3><p className="execution-status">该模块已由用户终止，保留历史记录且不会再发送消息或启动 Codex。</p></section> : null}
        {selected.phase === 'WAITING_FOR_ACCEPTANCE' ? <RelayAcceptancePanel blockedByUnknown={recoveryMessages.some((message) => message.moduleId === selected.id)} busy={busy} onAccept={acceptSelectedModule} onSubmitFeedback={submitAcceptanceFeedback} /> : null}
        <RelayModuleActions phase={selected.phase} stopAfterTurn={selected.stopAfterTurn} blockedByUnknown={recoveryMessages.some((message) => message.moduleId === selected.id)} busy={busy} onTerminate={terminateSelectedModule} />
        <CodexCommunicationPanel cycles={codexCycles} />
        {codexInputRequests.map((request) => <CodexHumanInputPanel key={request.id} request={request} stopAfterTurn={selected.stopAfterTurn} onSubmit={(answers) => submitCodexInput(request.id, answers)} />)}
        <section className="form-section conversation"><h3>常驻 ChatGPT 对话</h3><div className="message-history" aria-live="polite">{messages.filter((message) => message.direction === 'TO_CHATGPT' || message.direction === 'FROM_CHATGPT').length === 0 ? <p className="empty light">历史将在本次传话开始后显示。</p> : messages.filter((message) => message.direction === 'TO_CHATGPT' || message.direction === 'FROM_CHATGPT').map((message) => <article className={`message ${message.direction.toLowerCase()} ${message.kind.toLowerCase()}`} key={message.id}><header><strong>{message.direction === 'FROM_CHATGPT' ? 'ChatGPT' : '你 → ChatGPT'}</strong><span>{message.kind === 'MANUAL' ? '手动' : '自动化'} · {message.deliveryState}</span></header><pre>{message.text}</pre>{message.direction === 'TO_CHATGPT' && message.deliveryState === 'UNKNOWN' ? <div className="uncertain-delivery"><p>这条消息的送达结果不确定，系统没有自动重发。</p><button className="secondary" disabled={busy} type="button" onClick={() => void retryUnknownMessage(message.id)}>明确重发这条消息</button><button className="secondary" disabled={busy} type="button" onClick={() => void continueUnknownMessageWithoutResend(message.id)}>不重发并继续</button></div> : null}</article>)}</div>{!terminalPhases.has(selected.phase) ? <form className="composer" onSubmit={send}><div className="mode-switch"><button className={kind === 'MANUAL' ? 'selected' : ''} type="button" onClick={() => setKind('MANUAL')}>手动聊天</button><button className={kind === 'AUTOMATION' ? 'selected' : ''} type="button" onClick={() => setKind('AUTOMATION')}>发送自动化请求</button></div><textarea rows={4} value={text} onChange={(event) => setText(event.target.value)} placeholder={kind === 'MANUAL' ? '这条消息只用于和 ChatGPT 沟通，不解析控制块。' : '明确要求 ChatGPT 给出下一个控制块；仅这条消息的回复会参与自动化。'} /><button className="primary" disabled={busy} type="submit">{busy ? '正在处理…' : '发送给 ChatGPT'}</button></form> : null}</section>
      </>}
    </section>
  </main>;
}
