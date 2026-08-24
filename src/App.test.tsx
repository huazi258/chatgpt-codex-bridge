import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';

function module(overrides: Record<string, unknown> = {}) {
  return { id: 'one', name: 'Bridge UI', workingDirectory: 'G:\\project', maxCycles: 12, maxRuntimeMinutes: 240, retryTemplate: 'retry', phase: 'READY', stopAfterTurn: false, invalidReplyCount: 0, startedCycles: 0, moduleStartedAt: null, ...overrides };
}

let modules: ReturnType<typeof module>[];
let messages: Record<string, unknown>[];
let cycles: Record<string, unknown>[];
let recoveryMessages: Record<string, unknown>[];
let snapshot: Record<string, unknown>;
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => undefined) }));

invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
  switch (command) {
    case 'list_relay_modules': return modules;
    case 'get_chatgpt_pairing': return { endpoint: 'ws://127.0.0.1:8765', pairingSecret: 'secret', paired: true };
    case 'list_relay_messages': return messages;
    case 'list_relay_codex_cycles': return cycles;
    case 'list_relay_recovery_messages': return recoveryMessages;
    case 'get_relay_channel_snapshot': return snapshot;
    case 'get_relay_codex_thread_state': return { moduleId: 'one', workingDirectory: 'G:\\project', summary: '需要恢复', allowedActions: [{ type: 'RETRY_RESUME' }] };
    case 'create_relay_module': {
      const input = args?.input as Record<string, unknown>;
      const created = module({ id: 'two', name: input.name, workingDirectory: input.workingDirectory, retryTemplate: input.retryTemplate });
      modules = [...modules, created];
      return created;
    }
    case 'delete_relay_module': modules = modules.filter((item) => item.id !== args?.moduleId); return;
    case 'queue_relay_message': return;
    case 'retry_unknown_relay_message': return;
    case 'continue_unknown_relay_message_without_resend': return;
    case 'accept_relay_module': return;
    case 'submit_relay_acceptance_feedback': return;
    case 'terminate_relay_module': return;
    case 'recover_relay_codex': return;
    case 'open_relay_working_directory': return;
    default: throw new Error(`Unexpected Tauri command: ${command}`);
  }
});

describe('会话工作台', () => {
  beforeEach(() => {
    modules = [module()];
    messages = [];
    cycles = [];
    recoveryMessages = [];
    snapshot = { chatgpt: { status: 'IDLE', recoveryBlockerCount: 0 }, codex: { status: 'IDLE' } };
    invoke.mockClear();
    localStorage.clear();
  });
  afterEach(cleanup);

  it('已有会话时仍可创建并选中新会话，默认 retry template 包含 CODEX_INPUT', async () => {
    render(<App />);
    await screen.findByLabelText('打开“Bridge UI”菜单');
    fireEvent.click(screen.getByTitle('新建会话'));
    fireEvent.click(screen.getByRole('button', { name: /高级设置/ }));
    expect((screen.getByLabelText('Retry template') as HTMLTextAreaElement).value).toContain('@@@CODEX_INPUT@@@');
    fireEvent.change(screen.getByLabelText('会话名称'), { target: { value: '新会话' } });
    fireEvent.change(screen.getByLabelText('Codex 工作目录'), { target: { value: 'G:\\new-project' } });
    fireEvent.click(screen.getByRole('button', { name: '创建会话' }));
    expect((await screen.findAllByText('新会话')).length).toBeGreaterThan(0);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('create_relay_module', expect.objectContaining({ input: expect.objectContaining({ name: '新会话' }) })));
  });

  it('MANUAL 与 AUTOMATION 保持正确的入队分类', async () => {
    render(<App />);
    const composer = await screen.findByPlaceholderText('输入消息…');
    fireEvent.change(composer, { target: { value: '手动消息' } });
    fireEvent.click(screen.getByRole('button', { name: '发送' }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('queue_relay_message', { moduleId: 'one', kind: 'MANUAL', text: '手动消息' }));
    fireEvent.change(screen.getByLabelText('消息模式'), { target: { value: 'AUTOMATION' } });
    fireEvent.change(composer, { target: { value: '自动化消息' } });
    fireEvent.click(screen.getByRole('button', { name: '发送' }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('queue_relay_message', { moduleId: 'one', kind: 'AUTOMATION', text: '自动化消息' }));
  });

  it('UNKNOWN 可明确重发或不重发继续', async () => {
    recoveryMessages = [{ messageId: 'unknown-1', moduleId: 'one', moduleName: 'Bridge UI', sequenceNumber: 4, kind: 'AUTOMATION', createdAt: '2026-08-24T00:00:00Z' }];
    render(<App />);
    await screen.findByText('送达结果不确定');
    fireEvent.click(screen.getByRole('button', { name: '明确重发' }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('retry_unknown_relay_message', { messageId: 'unknown-1' }));
    fireEvent.click(screen.getByRole('button', { name: '不重发并继续' }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('continue_unknown_relay_message_without_resend', { messageId: 'unknown-1' }));
  });

  it('验收 Modal 可关闭重开，并提交反馈', async () => {
    modules = [module({ phase: 'WAITING_FOR_ACCEPTANCE' })];
    render(<App />);
    await screen.findByRole('dialog', { name: '等待人工验收' });
    fireEvent.click(screen.getByRole('button', { name: '关闭' }));
    fireEvent.click(screen.getByRole('button', { name: '等待验收' }));
    await screen.findByRole('dialog', { name: '等待人工验收' });
    fireEvent.click(screen.getByRole('button', { name: '继续处理' }));
    fireEvent.change(screen.getByPlaceholderText('输入反馈…'), { target: { value: '请补充验证' } });
    fireEvent.click(screen.getByRole('button', { name: '提交并继续' }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('submit_relay_acceptance_feedback', { moduleId: 'one', text: '请补充验证' }));
  });

  it('终止确认仍调用既有状态机接口', async () => {
    render(<App />);
    fireEvent.click(await screen.findByTitle('终止当前会话'));
    fireEvent.click(screen.getByRole('button', { name: '终止会话' }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('terminate_relay_module', { moduleId: 'one' }));
  });

  it('Codex recovery 面板仍可调用恢复接口', async () => {
    modules = [module({ phase: 'RECOVERY_REQUIRED' })];
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: '重新尝试继续此对话' }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('recover_relay_codex', { moduleId: 'one', action: { type: 'RETRY_RESUME' } }));
  });

  it('删除调用后自动进入空状态', async () => {
    render(<App />);
    fireEvent.click(await screen.findByLabelText('打开“Bridge UI”菜单'));
    fireEvent.click(screen.getByRole('button', { name: '删除会话' }));
    fireEvent.click(screen.getByRole('button', { name: '删除会话' }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('delete_relay_module', { moduleId: 'one' }));
    await screen.findByText('还没有会话。');
  });

  it('RECOVERY_BLOCKED 顶栏不会显示为健康状态', async () => {
    snapshot = { chatgpt: { status: 'RECOVERY_BLOCKED', recoveryBlockerCount: 1 }, codex: { status: 'IDLE' } };
    render(<App />);
    const button = await screen.findByRole('button', { name: '● ChatGPT 待恢复' });
    expect(button.className).toContain('warning');
    expect(button.className).not.toContain('healthy');
  });
});
