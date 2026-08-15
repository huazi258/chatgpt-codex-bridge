import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { FormEvent, useEffect, useMemo, useState } from 'react';
import {
  CodexExecutionEvent,
  CodexTurnResult,
  ChatGptBridgeStatus,
  ChatGptPairingInfo,
  emptyDraft,
  ModuleDraft,
  ModuleRecord
} from './types';

function asDraft(module: ModuleRecord): ModuleDraft {
  return {
    name: module.name,
    repositoryPath: module.repositoryPath,
    targetBranch: module.targetBranch,
    chatgptTabId: String(module.chatgptTabId),
    maxRounds: String(module.budget.maxRounds),
    moduleTimeoutMinutes: String(module.budget.moduleTimeoutMinutes),
    globalTimeoutMinutes: String(module.budget.globalTimeoutMinutes)
  };
}

function validateDraft(draft: ModuleDraft): string | null {
  if (!draft.name.trim() || !draft.repositoryPath.trim() || !draft.targetBranch.trim()) {
    return '请填写模块名称、仓库目录和目标分支。';
  }
  const values = [draft.chatgptTabId, draft.maxRounds, draft.moduleTimeoutMinutes, draft.globalTimeoutMinutes];
  if (values.some((value) => !/^\d+$/.test(value) || Number(value) <= 0)) {
    return 'ChatGPT 标签页 ID 和所有预算必须为正整数。';
  }
  return null;
}

export default function App() {
  const [modules, setModules] = useState<ModuleRecord[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [draft, setDraft] = useState<ModuleDraft>(emptyDraft);
  const [notice, setNotice] = useState('正在加载本地模块…');
  const [busy, setBusy] = useState(false);
  const [codexTask, setCodexTask] = useState(
    'Reply exactly CODEX_ADAPTER_SMOKE_OK. Do not run commands, inspect files, modify files, commit, or push.'
  );
  const [codexEvent, setCodexEvent] = useState<CodexExecutionEvent | null>(null);
  const [codexResult, setCodexResult] = useState<CodexTurnResult | null>(null);
  const [pairing, setPairing] = useState<ChatGptPairingInfo | null>(null);
  const [chatgptStatus, setChatgptStatus] = useState<ChatGptBridgeStatus | null>(null);

  const selected = useMemo(
    () => modules.find((module) => module.id === selectedId) ?? null,
    [modules, selectedId]
  );

  async function refresh(preferredId?: string | null) {
    const records = await invoke<ModuleRecord[]>('list_inactive_modules');
    setModules(records);
    const nextId = preferredId ?? selectedId;
    const next = records.find((module) => module.id === nextId) ?? records[0] ?? null;
    setSelectedId(next?.id ?? null);
    setDraft(next ? asDraft(next) : emptyDraft);
  }

  useEffect(() => {
    refresh()
      .then(() => setNotice('本地状态已就绪。模块尚未运行。'))
      .catch((error) => setNotice(`无法读取本地数据库：${String(error)}`));
  }, []);

  async function refreshPairing() {
    const info = await invoke<ChatGptPairingInfo>('get_chatgpt_pairing');
    setPairing(info);
  }

  useEffect(() => {
    refreshPairing().catch((error) => setNotice(`无法启动 ChatGPT 本机桥接：${String(error)}`));
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<CodexExecutionEvent>('codex-status', (event) => {
      setCodexEvent(event.payload);
    }).then((unsubscribe) => {
      unlisten = unsubscribe;
    });
    return () => unlisten?.();
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<ChatGptBridgeStatus>('chatgpt-status', (event) => {
      setChatgptStatus(event.payload);
      refreshPairing().catch(() => undefined);
    }).then((unsubscribe) => {
      unlisten = unsubscribe;
    });
    return () => unlisten?.();
  }, []);

  function updateField(field: keyof ModuleDraft, value: string) {
    setDraft((current) => ({ ...current, [field]: value }));
  }

  function selectModule(module: ModuleRecord) {
    setSelectedId(module.id);
    setDraft(asDraft(module));
    setNotice(`已打开“${module.name}”。它保持为未运行状态。`);
  }

  function newModule() {
    setSelectedId(null);
    setDraft(emptyDraft);
    setNotice('正在创建一个未运行模块。保存前不会启动任何自动化。');
  }

  async function save(event: FormEvent) {
    event.preventDefault();
    const validationError = validateDraft(draft);
    if (validationError) {
      setNotice(validationError);
      return;
    }

    setBusy(true);
    try {
      const payload = {
        name: draft.name.trim(),
        repositoryPath: draft.repositoryPath.trim(),
        targetBranch: draft.targetBranch.trim(),
        chatgptTabId: Number(draft.chatgptTabId),
        maxRounds: Number(draft.maxRounds),
        moduleTimeoutMinutes: Number(draft.moduleTimeoutMinutes),
        globalTimeoutMinutes: Number(draft.globalTimeoutMinutes)
      };
      const saved = selected
        ? await invoke<ModuleRecord>('update_inactive_module', { id: selected.id, input: payload })
        : await invoke<ModuleRecord>('create_inactive_module', { input: payload });
      await refresh(saved.id);
      setNotice(`已保存“${saved.name}”。仓库尚未被执行或修改。`);
    } catch (error) {
      setNotice(`保存失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  }

  async function removeSelected() {
    if (!selected) return;
    setBusy(true);
    try {
      await invoke('delete_inactive_module', { id: selected.id });
      await refresh(null);
      setNotice(`已删除未运行模块“${selected.name}”。未改动其仓库。`);
    } catch (error) {
      setNotice(`删除失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  }

  async function runControlledCodexTurn() {
    if (!selected) {
      setNotice('请先创建并保存一个未运行模块。');
      return;
    }
    if (!codexTask.trim()) {
      setNotice('请填写要发送给 Codex 的任务。');
      return;
    }
    setBusy(true);
    setCodexResult(null);
    setCodexEvent({
      moduleId: selected.id,
      phase: 'STARTING',
      statusLine: '正在准备受控 Codex 回合…'
    });
    try {
      const result = await invoke<CodexTurnResult>('execute_controlled_codex_turn', {
        moduleId: selected.id,
        task: codexTask
      });
      setCodexResult(result);
      setNotice(result.status === 'COMPLETED' ? 'Codex 回合已完成。' : 'Codex 回合已暂停，等待后续用户处理。');
    } catch (error) {
      setCodexEvent({
        moduleId: selected.id,
        phase: 'FAILED',
        statusLine: `Codex App Server 错误：${String(error)}`
      });
      setNotice('Codex 回合未能启动或异常结束。');
    } finally {
      setBusy(false);
    }
  }

  async function sendProtocolBootstrap() {
    if (!pairing?.paired) {
      setNotice('请先在 Chrome 扩展中绑定专用 ChatGPT 标签页。');
      return;
    }
    const moduleName = selected?.name ?? '未命名模块';
    const message = `你是“${moduleName}”模块的规划与 Review 决策者。每次自动化回复可以先写给用户看的自然语言说明，然后必须以且仅以一个 \`\`\`json 代码块结束。JSON 必须使用以下字段：state、module、reason、codex_prompt、acceptance_criteria、review_scope、requires_user_input。state 只能是 NEXT_TASK、MODULE_DONE、PAUSE、BLOCKED。NEXT_TASK 必须提供完整 codex_prompt 和至少一条 acceptance_criteria；其他状态不得提供 codex_prompt。现在请只回复一个 PAUSE 协议包：代码块内必须只含 state、module、reason、acceptance_criteria、review_scope、requires_user_input 六个字段，绝对不要输出 codex_prompt 字段。`;
    setBusy(true);
    try {
      await invoke('send_chatgpt_message', { text: message });
      setNotice('协议引导已发送，正在等待绑定 ChatGPT 标签页的回复。');
    } catch (error) {
      setNotice(`无法发送协议引导：${String(error)}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="shell">
      <aside className="sidebar">
        <div>
          <p className="eyebrow">LOCAL ORCHESTRATOR</p>
          <h1>模块准备区</h1>
          <p className="muted">此阶段只保存配置；不会访问 ChatGPT、Codex 或 Git。</p>
        </div>
        <button className="primary full" onClick={newModule} disabled={busy}>新建模块</button>
        <nav aria-label="未运行模块">
          <p className="section-label">未运行模块 · {modules.length}</p>
          {modules.length === 0 ? <p className="empty">还没有保存的模块。</p> : modules.map((module) => (
            <button
              className={`module-card ${module.id === selectedId ? 'selected' : ''}`}
              key={module.id}
              onClick={() => selectModule(module)}
              disabled={busy}
            >
              <strong>{module.name}</strong>
              <span>{module.targetBranch}</span>
            </button>
          ))}
        </nav>
      </aside>

      <section className="workspace">
        <header>
          <div>
            <p className="eyebrow">TASK 3 · CODEX EXECUTION ADAPTER</p>
            <h2>{selected ? '编辑未运行模块' : '新建未运行模块'}</h2>
          </div>
          <span className="status-pill">INACTIVE</span>
        </header>

        <p className="notice" role="status">{notice}</p>

        <form onSubmit={save}>
          <section className="form-section">
            <h3>工作范围</h3>
            <label>模块名称<input value={draft.name} onChange={(event) => updateField('name', event.target.value)} placeholder="例如：任务编排 MVP" /></label>
            <label>仓库目录<input value={draft.repositoryPath} onChange={(event) => updateField('repositoryPath', event.target.value)} placeholder="G:\\projects\\my-repository" /></label>
            <label>目标分支<input value={draft.targetBranch} onChange={(event) => updateField('targetBranch', event.target.value)} placeholder="main" /></label>
            <label>ChatGPT 专用标签页 ID<input inputMode="numeric" value={draft.chatgptTabId} onChange={(event) => updateField('chatgptTabId', event.target.value)} placeholder="例如：123456" /></label>
          </section>

          <section className="form-section">
            <h3>自动化预算</h3>
            <div className="budget-grid">
              <label>最大任务轮次<input inputMode="numeric" value={draft.maxRounds} onChange={(event) => updateField('maxRounds', event.target.value)} /></label>
              <label>模块最长时间（分钟）<input inputMode="numeric" value={draft.moduleTimeoutMinutes} onChange={(event) => updateField('moduleTimeoutMinutes', event.target.value)} /></label>
              <label>全局最长时间（分钟）<input inputMode="numeric" value={draft.globalTimeoutMinutes} onChange={(event) => updateField('globalTimeoutMinutes', event.target.value)} /></label>
            </div>
          </section>

          <section className="form-section execution-section">
            <div className="execution-heading">
              <div>
                <h3>Codex App Server 受控回合</h3>
                <p>通过本地 stdio 启动 `codex app-server`。默认任务是无副作用的连接验证。</p>
              </div>
              <span className={`execution-phase ${codexEvent?.phase.toLowerCase() ?? 'idle'}`}>
                {codexEvent?.phase ?? 'IDLE'}
              </span>
            </div>
            <label>Codex 任务<textarea value={codexTask} onChange={(event) => setCodexTask(event.target.value)} rows={4} /></label>
            <p className="execution-status">{codexEvent?.statusLine ?? '尚未开始。'}</p>
            {codexResult && (
              <div className="execution-result">
                <strong>最终摘要 · {codexResult.status}</strong>
                <pre>{codexResult.summary}</pre>
                <small>thread {codexResult.threadId ?? '—'} · turn {codexResult.turnId ?? '—'}</small>
              </div>
            )}
            <button className="secondary" type="button" onClick={runControlledCodexTurn} disabled={busy || !selected}>
              {busy ? 'Codex 正在运行…' : '运行受控 Codex 回合'}
            </button>
          </section>

          <section className="form-section chatgpt-section">
            <div className="execution-heading">
              <div>
                <h3>ChatGPT 专用标签页配对</h3>
                <p>扩展仅连接到本机回环地址；密钥只在本次桌面应用运行期间有效。</p>
              </div>
              <span className={`execution-phase ${pairing?.paired ? 'completed' : 'idle'}`}>
                {pairing?.paired ? 'PAIRED' : 'UNPAIRED'}
              </span>
            </div>
            <label>本机地址<input readOnly value={pairing?.endpoint ?? '正在启动…'} /></label>
            <label>一次性配对密钥<input readOnly value={pairing?.pairingSecret ?? '正在生成…'} /></label>
            <p className="execution-status">{chatgptStatus?.detail ?? '复制密钥到 Chrome 扩展，选择当前已登录的 ChatGPT 标签页并配对。'}</p>
            {chatgptStatus?.protocolState && <p className="protocol-result">协议已验证：{chatgptStatus.protocolState}</p>}
            <div className="inline-actions">
              <button className="secondary" type="button" onClick={() => refreshPairing().catch((error) => setNotice(String(error)))} disabled={busy}>刷新配对状态</button>
              <button className="secondary" type="button" onClick={sendProtocolBootstrap} disabled={busy || !pairing?.paired}>发送协议引导</button>
            </div>
          </section>

          <footer className="actions">
            {selected && <button className="danger" type="button" onClick={removeSelected} disabled={busy}>删除模块</button>}
            <button className="primary" type="submit" disabled={busy}>{busy ? '正在保存…' : '保存未运行模块'}</button>
          </footer>
        </form>
      </section>
    </main>
  );
}
