import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { FormEvent, useEffect, useMemo, useState } from 'react';

type RelayKind = 'MANUAL' | 'AUTOMATION';

interface PairingInfo { endpoint: string; pairingSecret: string; paired: boolean; }
interface BridgeStatus { phase: string; detail: string; }
interface RelayModule {
  id: string; name: string; workingDirectory: string; maxCycles: number; maxRuntimeMinutes: number;
  retryTemplate: string; phase: string; codexThreadId?: string; invalidReplyCount: number; startedCycles: number;
}
interface RelayMessage {
  id: string; sequenceNumber: number; direction: 'TO_CHATGPT' | 'FROM_CHATGPT' | 'TO_CODEX' | 'FROM_CODEX';
  kind: 'MANUAL' | 'AUTOMATION' | 'SYSTEM'; text: string; deliveryState: string;
}

const defaultRetry = '请根据既定格式，在回复最后且仅输出一个有效控制块：@@@CODEX_PROMPT@@@、@@@MODULE_DONE@@@、@@@BLOCKED@@@ 或正在等待输入时的 @@@CODEX_INPUT@@@。';

export default function App() {
  const [modules, setModules] = useState<RelayModule[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [messages, setMessages] = useState<RelayMessage[]>([]);
  const [pairing, setPairing] = useState<PairingInfo | null>(null);
  const [bridge, setBridge] = useState<BridgeStatus | null>(null);
  const [notice, setNotice] = useState('正在加载本地传话状态…');
  const [busy, setBusy] = useState(false);
  const [draft, setDraft] = useState({ name: '', workingDirectory: '', maxCycles: '12', maxRuntimeMinutes: '240', retryTemplate: defaultRetry });
  const [text, setText] = useState('');
  const [kind, setKind] = useState<RelayKind>('MANUAL');
  const selected = useMemo(() => modules.find((item) => item.id === selectedId) ?? null, [modules, selectedId]);

  async function refreshModules(preferredId = selectedId) {
    const next = await invoke<RelayModule[]>('list_relay_modules');
    setModules(next);
    setSelectedId((next.find((item) => item.id === preferredId) ?? next[0] ?? null)?.id ?? null);
  }
  async function refreshMessages(moduleId = selectedId) {
    if (!moduleId) return setMessages([]);
    setMessages(await invoke<RelayMessage[]>('list_relay_messages', { moduleId }));
  }
  async function refreshPairing() { setPairing(await invoke<PairingInfo>('get_chatgpt_pairing')); }

  useEffect(() => {
    Promise.all([refreshModules(), refreshPairing()])
      .then(() => setNotice('本地传话状态已就绪。先在 Chrome 扩展中绑定当前 ChatGPT 对话。'))
      .catch((error) => setNotice(`初始化失败：${String(error)}`));
  }, []);
  useEffect(() => { void refreshMessages().catch((error) => setNotice(`无法读取消息历史：${String(error)}`)); }, [selectedId]);
  useEffect(() => {
    let stopStatus: (() => void) | undefined;
    let stopControl: (() => void) | undefined;
    void listen<BridgeStatus>('chatgpt-status', (event) => {
      setBridge(event.payload); void refreshPairing().catch(() => undefined); void refreshMessages().catch(() => undefined);
    }).then((unsubscribe) => { stopStatus = unsubscribe; });
    void listen<{ type: string; reason?: string }>('relay-control', (event) => {
      setNotice(`已处理 ChatGPT 控制回复：${event.payload.type}${event.payload.reason ? `：${event.payload.reason}` : ''}`);
      void refreshModules().catch(() => undefined); void refreshMessages().catch(() => undefined);
    }).then((unsubscribe) => { stopControl = unsubscribe; });
    return () => { stopStatus?.(); stopControl?.(); };
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
      await refreshModules(module.id);
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
      setNotice(kind === 'MANUAL' ? '手动消息已入队；其回复只展示，不会触发 Codex。' : '自动化请求已入队；其对应回复将按控制块规则处理。');
      await refreshMessages(selected.id);
    } catch (error) { setNotice(`发送失败：${String(error)}`); } finally { setBusy(false); }
  }

  return <main className="shell relay-shell">
    <aside className="sidebar">
      <div><p className="eyebrow">CONVERSATION RELAY V2</p><h1>传话模块</h1><p className="muted">一个模块对应一个 ChatGPT 对话和一个由中间件持有的 Codex 对话。</p></div>
      <p className={`bridge-state ${pairing?.paired ? 'online' : 'offline'}`}>{pairing?.paired ? 'ChatGPT 已连接' : 'ChatGPT 未连接'}</p>
      <nav aria-label="传话模块列表"><p className="section-label">模块 · {modules.length}</p>{modules.length === 0 ? <p className="empty">还没有传话模块。</p> : modules.map((module) => <button className={`module-card ${module.id === selectedId ? 'selected' : ''}`} key={module.id} onClick={() => { setSelectedId(module.id); setNotice(`已打开“${module.name}”。`); }} disabled={busy}><strong>{module.name}</strong><span>{module.phase}</span></button>)}</nav>
    </aside>
    <section className="workspace relay-workspace">
      <header><div><p className="eyebrow">受控文本传话</p><h2>{selected?.name ?? '创建传话模块'}</h2></div><span className="status-pill">{selected?.phase ?? 'READY'}</span></header>
      <p className="notice" role="status">{notice}</p>
      <section className="form-section connection-card"><div><h3>ChatGPT 浏览器连接</h3><p className="execution-status">{bridge?.detail ?? '在 Chrome 扩展中选择当前已登录的 ChatGPT 对话后配对。'}</p></div><label>本机地址<input readOnly value={pairing?.endpoint ?? '正在启动…'} /></label><label>一次性配对密钥<input readOnly value={pairing?.pairingSecret ?? '正在生成…'} /></label><button className="secondary" type="button" onClick={() => void refreshPairing().catch((error) => setNotice(String(error)))} disabled={busy}>刷新连接状态</button></section>
      {!selected ? <form className="form-section" onSubmit={createModule}><h3>新建模块</h3><label>模块名称<input value={draft.name} onChange={(event) => setDraft({ ...draft, name: event.target.value })} placeholder="例如：中间件 V2 实现" /></label><label>Codex 工作目录<input value={draft.workingDirectory} onChange={(event) => setDraft({ ...draft, workingDirectory: event.target.value })} placeholder="G:\\projects\\your-project" /></label><div className="budget-grid"><label>最大自动循环次数<input inputMode="numeric" value={draft.maxCycles} onChange={(event) => setDraft({ ...draft, maxCycles: event.target.value })} /></label><label>模块最长时间（分钟）<input inputMode="numeric" value={draft.maxRuntimeMinutes} onChange={(event) => setDraft({ ...draft, maxRuntimeMinutes: event.target.value })} /></label></div><label>协议重试模板<textarea rows={3} value={draft.retryTemplate} onChange={(event) => setDraft({ ...draft, retryTemplate: event.target.value })} /></label><button className="primary" disabled={busy} type="submit">创建传话模块</button></form> : <>
        <section className="form-section relay-summary"><div><h3>模块状态</h3><p className="execution-status">工作目录：{selected.workingDirectory}</p></div><p className="execution-status">已开始循环：{selected.startedCycles} / {selected.maxCycles} · 最长运行：{selected.maxRuntimeMinutes} 分钟 · 无效自动化回复：{selected.invalidReplyCount}</p>{selected.codexThreadId ? <p className="protocol-result">Codex 对话：{selected.codexThreadId}</p> : <p className="execution-status">首个有效 CODEX_PROMPT 到来后才会创建 Codex 对话。</p>}</section>
        <section className="form-section conversation"><h3>常驻 ChatGPT 对话</h3><div className="message-history" aria-live="polite">{messages.length === 0 ? <p className="empty light">历史将在本次传话开始后显示。</p> : messages.map((message) => <article className={`message ${message.direction.toLowerCase()} ${message.kind.toLowerCase()}`} key={message.id}><header><strong>{message.direction === 'FROM_CHATGPT' ? 'ChatGPT' : message.direction === 'TO_CODEX' ? 'Codex 提示词' : '你 → ChatGPT'}</strong><span>{message.kind === 'MANUAL' ? '手动' : '自动化'} · {message.deliveryState}</span></header><pre>{message.text}</pre></article>)}</div><form className="composer" onSubmit={send}><div className="mode-switch"><button className={kind === 'MANUAL' ? 'selected' : ''} type="button" onClick={() => setKind('MANUAL')}>手动聊天</button><button className={kind === 'AUTOMATION' ? 'selected' : ''} type="button" onClick={() => setKind('AUTOMATION')}>发送自动化请求</button></div><textarea rows={4} value={text} onChange={(event) => setText(event.target.value)} placeholder={kind === 'MANUAL' ? '这条消息只用于和 ChatGPT 沟通，不解析控制块。' : '明确要求 ChatGPT 给出下一个控制块；仅这条消息的回复会参与自动化。'} /><button className="primary" disabled={busy} type="submit">{busy ? '正在处理…' : '发送给 ChatGPT'}</button></form></section>
      </>}
    </section>
  </main>;
}
