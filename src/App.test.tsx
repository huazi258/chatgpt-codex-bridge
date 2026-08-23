import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';
import { CodexCommunicationPanel } from './components/CodexCommunicationPanel';
import { CodexCycleCard } from './components/CodexCycleCard';
import { GlobalChannelStatus } from './components/GlobalChannelStatus';
import type { RelayChannelSnapshot, RelayCodexCycle } from './relay-observability';

let modules = [
  { id: 'existing', name: '原有模块', workingDirectory: 'G:\\projects\\existing', maxCycles: 12, maxRuntimeMinutes: 240, retryTemplate: '重试', phase: 'READY', stopAfterTurn: false, invalidReplyCount: 0, startedCycles: 0 },
];
let recoveryMessages: Array<{ messageId: string; moduleId: string; moduleName: string; sequenceNumber: number; kind: string; createdAt: string }> = [];
let codexCycles: RelayCodexCycle[] = [];
let relayMessages: Array<{ id: string; sequenceNumber: number; direction: 'TO_CHATGPT' | 'FROM_CHATGPT' | 'TO_CODEX' | 'FROM_CODEX'; kind: 'MANUAL' | 'AUTOMATION' | 'SYSTEM'; text: string; deliveryState: string }> = [];
let terminateError: Error | null = null;
let channelSnapshot: RelayChannelSnapshot = {
  chatgpt: { status: 'IDLE', recoveryBlockerCount: 0 },
  codex: { status: 'IDLE' },
};

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

invoke.mockImplementation(async (command: string, args?: { input?: { name: string; workingDirectory: string } }) => {
  if (command === 'list_relay_modules') return modules;
  if (command === 'get_chatgpt_pairing') return { endpoint: 'ws://127.0.0.1:17384', pairingSecret: 'secret', paired: false };
  if (command === 'list_relay_messages') return relayMessages;
  if (command === 'list_relay_recovery_messages') return recoveryMessages;
  if (command === 'list_relay_codex_cycles') return codexCycles;
  if (command === 'get_relay_channel_snapshot') return channelSnapshot;
  if (command === 'create_relay_module') {
    const input = args?.input;
    if (!input) throw new Error('missing module input');
    const created = { id: 'created', name: input.name, workingDirectory: input.workingDirectory, maxCycles: 12, maxRuntimeMinutes: 240, retryTemplate: '重试', phase: 'READY', stopAfterTurn: false, invalidReplyCount: 0, startedCycles: 0 };
    modules = [created, ...modules];
    return created;
  }
  if (command === 'accept_relay_module' || command === 'submit_relay_acceptance_feedback') return undefined;
  if (command === 'terminate_relay_module') {
    if (terminateError) throw terminateError;
    return undefined;
  }
  throw new Error(`unexpected command: ${command}`);
});

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => undefined) }));

describe('传话模块创建入口', () => {
  beforeEach(() => {
    modules = [
      { id: 'existing', name: '原有模块', workingDirectory: 'G:\\projects\\existing', maxCycles: 12, maxRuntimeMinutes: 240, retryTemplate: '重试', phase: 'READY', stopAfterTurn: false, invalidReplyCount: 0, startedCycles: 0 },
    ];
    recoveryMessages = [];
    codexCycles = [];
    relayMessages = [];
    terminateError = null;
    channelSnapshot = {
      chatgpt: { status: 'IDLE', recoveryBlockerCount: 0 },
      codex: { status: 'IDLE' },
    };
    invoke.mockClear();
  });

  afterEach(cleanup);

  it('保留原有模块时仍可新建，并在成功后打开新模块', async () => {
    render(<App />);
    await screen.findByRole('heading', { name: '原有模块' });

    fireEvent.click(screen.getByRole('button', { name: '新建模块' }));
    expect(screen.getByRole('heading', { name: '创建传话模块' })).toBeTruthy();

    fireEvent.change(screen.getByLabelText('模块名称'), { target: { value: '第二个模块' } });
    fireEvent.change(screen.getByLabelText('Codex 工作目录'), { target: { value: 'G:\\projects\\second' } });
    fireEvent.click(screen.getByRole('button', { name: '创建传话模块' }));

    await screen.findByRole('heading', { name: '第二个模块' });
    expect(screen.getByRole('button', { name: /原有模块/ })).toBeTruthy();
    expect(screen.getByRole('button', { name: /第二个模块/ })).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: /原有模块/ }));
    await waitFor(() => expect(screen.getByRole('heading', { name: '原有模块' })).toBeTruthy());
  });

  it('显示其他模块造成的全部不确定送达阻塞，并提供不重发继续入口', async () => {
    recoveryMessages = [
      { messageId: 'unknown-a', moduleId: 'module-a', moduleName: '模块 A', sequenceNumber: 4, kind: 'AUTOMATION', createdAt: '2026-08-17T07:45:37Z' },
      { messageId: 'unknown-c', moduleId: 'module-c', moduleName: '模块 C', sequenceNumber: 2, kind: 'MANUAL', createdAt: '2026-08-17T07:46:00Z' },
    ];
    render(<App />);

    await screen.findByRole('heading', { name: '原有模块' });
    expect(await screen.findByText('存在待人工处理的不确定送达消息')).toBeTruthy();
    expect(screen.getByText('模块 A · 第 4 条 · 自动化')).toBeTruthy();
    expect(screen.getByText('模块 C · 第 2 条 · 手动')).toBeTruthy();
    expect(screen.getAllByRole('button', { name: '明确重发这条消息' })).toHaveLength(2);
    expect(screen.getAllByRole('button', { name: '不重发并继续' })).toHaveLength(2);
  });

  it('在独立 Codex 面板展示已完成结果和其他模块的 ChatGPT 占用，不污染 ChatGPT 时间线', async () => {
    channelSnapshot = {
      chatgpt: {
        status: 'IN_FLIGHT',
        activeModuleId: 'other-module',
        activeModuleName: '占用模块',
        activeMessageId: 'other-message-7',
        activeKind: 'AUTOMATION',
        activePhase: '等待完成回复',
        recoveryBlockerCount: 0,
      },
      codex: { status: 'IDLE' },
    };
    codexCycles = [{
      ...completedCycle,
      moduleId: 'existing',
      blockReason: 'ChatGPT 通道当前被模块「占用模块」占用（消息 other-message-7）。',
    }];
    relayMessages = [
      { id: 'chatgpt-request', sequenceNumber: 1, direction: 'TO_CHATGPT', kind: 'AUTOMATION', text: '请执行验收。', deliveryState: 'DELIVERED' },
      { id: 'codex-prompt', sequenceNumber: 2, direction: 'TO_CODEX', kind: 'AUTOMATION', text: '这是内部 Codex 提示词。', deliveryState: 'DELIVERED' },
      { id: 'codex-result', sequenceNumber: 3, direction: 'FROM_CODEX', kind: 'SYSTEM', text: '这是内部 Codex 生命周期结果。', deliveryState: 'DELIVERED' },
    ];

    const { container } = render(<App />);
    await screen.findByRole('heading', { name: '原有模块' });

    expect(await screen.findByText('全局通道状态')).toBeTruthy();
    expect(screen.getByText('ChatGPT 通道：忙碌')).toBeTruthy();
    expect(screen.getByText('当前占用模块：占用模块')).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Codex 通讯' })).toBeTruthy();
    expect(screen.getByText('RELAY_E2E_OK')).toBeTruthy();
    expect(screen.getByText('阻塞原因：ChatGPT 通道当前被模块「占用模块」占用（消息 other-message-7）。')).toBeTruthy();
    expect(screen.getByRole('heading', { name: '常驻 ChatGPT 对话' })).toBeTruthy();
    expect(container.querySelector('.message-history')?.textContent).toContain('请执行验收。');
    expect(container.querySelector('.message-history')?.textContent).not.toContain('这是内部 Codex 提示词。');
    expect(container.querySelector('.message-history')?.textContent).not.toContain('这是内部 Codex 生命周期结果。');
    expect(container.querySelector('.message-history')?.textContent).not.toContain('RELAY_E2E_OK');
    expect(container.querySelector('.message-history')?.textContent).not.toContain('Codex 运行中');
  });
});

describe('模块验收与终止', () => {
  beforeEach(() => {
    modules = [{ id: 'existing', name: '原有模块', workingDirectory: 'G:\\projects\\existing', maxCycles: 12, maxRuntimeMinutes: 240, retryTemplate: '重试', phase: 'WAITING_FOR_ACCEPTANCE', stopAfterTurn: false, invalidReplyCount: 0, startedCycles: 1 }];
    recoveryMessages = [];
    codexCycles = [];
    relayMessages = [];
    terminateError = null;
    channelSnapshot = { chatgpt: { status: 'IDLE', recoveryBlockerCount: 0 }, codex: { status: 'IDLE' } };
    invoke.mockClear();
    vi.spyOn(window, 'confirm').mockReturnValue(true);
  });

  afterEach(() => {
    cleanup();
    vi.mocked(window.confirm).mockRestore();
  });

  it('等待验收时显示接受、反馈和终止动作；空反馈不提交，成功后清空', async () => {
    render(<App />);
    await screen.findByRole('heading', { name: '原有模块' });

    expect(screen.getByText('等待人工验收')).toBeTruthy();
    expect(screen.getByRole('button', { name: '接受并完成模块' })).toBeTruthy();
    const feedback = screen.getByLabelText('验收反馈');
    const submit = screen.getByRole('button', { name: '提交反馈并继续' });
    expect((submit as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(feedback, { target: { value: '请补充验收测试。' } });
    fireEvent.click(submit);

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('submit_relay_acceptance_feedback', { moduleId: 'existing', text: '请补充验收测试。' }));
    expect((feedback as HTMLTextAreaElement).value).toBe('');
    fireEvent.click(screen.getByRole('button', { name: '接受并完成模块' }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('accept_relay_module', { moduleId: 'existing' }));
  });

  it('终止需要二次确认，并在后端拒绝时显示中文提示', async () => {
    terminateError = new Error('请先处理本模块的不确定送达消息');
    render(<App />);
    await screen.findByRole('heading', { name: '原有模块' });
    fireEvent.click(screen.getByRole('button', { name: '终止模块' }));

    expect(window.confirm).toHaveBeenCalled();
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('terminate_relay_module', { moduleId: 'existing' }));
    expect(await screen.findByText(/终止模块失败：.*请先处理本模块的不确定送达消息/)).toBeTruthy();
  });

  it('已请求运行中终止时显示等待文案并禁用重复终止', async () => {
    modules = [{ ...modules[0], phase: 'CODEX_RUNNING', stopAfterTurn: true }];
    render(<App />);
    await screen.findByRole('heading', { name: '原有模块' });

    expect(screen.getByText('终止已请求，等待当前 Codex 回合结束')).toBeTruthy();
    expect((screen.getByRole('button', { name: '终止模块' }) as HTMLButtonElement).disabled).toBe(true);
  });

  it.each([
    ['COMPLETED', '已验收完成'],
    ['STOPPED', '已终止'],
  ])('终态 %s 显示终态提示并隐藏 composer 和模块动作', async (phase, label) => {
    modules = [{ ...modules[0], phase }];
    render(<App />);
    await screen.findByRole('heading', { name: '原有模块' });

    expect(screen.getByText(label)).toBeTruthy();
    expect(screen.queryByRole('button', { name: '发送给 ChatGPT' })).toBeNull();
    expect(screen.queryByRole('button', { name: '终止模块' })).toBeNull();
    expect(screen.queryByRole('button', { name: '接受并完成模块' })).toBeNull();
  });

  it('当前模块存在 UNKNOWN 时提示先恢复并禁用接受和终止', async () => {
    recoveryMessages = [{ messageId: 'unknown-existing', moduleId: 'existing', moduleName: '原有模块', sequenceNumber: 3, kind: 'AUTOMATION', createdAt: '2026-08-18T00:00:00Z' }];
    render(<App />);
    await screen.findByRole('heading', { name: '原有模块' });

    expect(screen.getAllByText('请先处理本模块的不确定送达消息')).toHaveLength(2);
    expect((screen.getByRole('button', { name: '接受并完成模块' }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole('button', { name: '终止模块' }) as HTMLButtonElement).disabled).toBe(true);
  });
});

const runningSnapshot: RelayChannelSnapshot = {
  chatgpt: {
    status: 'IN_FLIGHT',
    activeModuleId: 'module-a',
    activeModuleName: '模块 A',
    activeMessageId: 'chatgpt-message-17',
    activeKind: 'AUTOMATION',
    activePhase: '等待完成回复',
    recoveryBlockerCount: 0,
  },
  codex: {
    status: 'RUNNING',
    activeModuleId: 'module-b',
    activeModuleName: '模块 B',
    cycleNumber: 3,
    codexThreadId: 'thread-codex-3',
    codexTurnId: null,
    cycleStatus: 'CODEX_RUNNING',
  },
};

const completedCycle: RelayCodexCycle = {
  id: 'cycle-3',
  moduleId: 'module-b',
  cycleNumber: 3,
  status: 'WAITING_FOR_CHATGPT',
  promptText: '请实现 Relay E2E 检查。',
  codexThreadId: 'thread-codex-3',
  codexTurnId: null,
  resultText: 'RELAY_E2E_OK',
  outboundChatgptMessageId: 'chatgpt-message-18',
  errorText: null,
  createdAt: '2026-08-17T08:00:00Z',
  codexStartedAt: '2026-08-17T08:01:00Z',
  codexCompletedAt: '2026-08-17T08:02:00Z',
  relayQueuedAt: '2026-08-17T08:02:01Z',
  relayDeliveredAt: null,
  updatedAt: '2026-08-17T08:02:01Z',
  blockReason: 'ChatGPT 通道当前被模块「模块 A」占用（消息 chatgpt-message-17）。',
};

describe('Codex 通讯可观测性组件', () => {
  afterEach(cleanup);

  it('展示忙碌 ChatGPT、recovery blocker 与运行中的 Codex 通道', () => {
    const { rerender } = render(<GlobalChannelStatus snapshot={runningSnapshot} />);

    expect(screen.getByText('ChatGPT 通道：忙碌')).toBeTruthy();
    expect(screen.getByText('当前占用模块：模块 A')).toBeTruthy();
    expect(screen.getByText('当前消息：chatgpt-message-17')).toBeTruthy();
    expect(screen.getByText('Codex 通道：运行中')).toBeTruthy();
    expect(screen.getByText('当前模块：模块 B')).toBeTruthy();
    expect(screen.getByText('Cycle：3')).toBeTruthy();
    expect(screen.getByText('Codex thread：thread-codex-3')).toBeTruthy();
    expect(screen.getByText('Codex turn：尚未获得')).toBeTruthy();

    rerender(<GlobalChannelStatus snapshot={{
      ...runningSnapshot,
      chatgpt: { ...runningSnapshot.chatgpt, status: 'RECOVERY_BLOCKED', recoveryBlockerCount: 2 },
    }} />);
    expect(screen.getByText('ChatGPT 通道：恢复阻塞')).toBeTruthy();
    expect(screen.getByText('待恢复 UNKNOWN：2 条')).toBeTruthy();
  });

  it('展示 cycle 的 prompt、thread、result、缺失 turn 与阻塞原因', () => {
    render(<CodexCycleCard cycle={completedCycle} />);

    expect(screen.getByText('Cycle 3 · 等待回传 ChatGPT')).toBeTruthy();
    expect(screen.getByText('Prompt 原文')).toBeTruthy();
    expect(screen.getByText(completedCycle.promptText)).toBeTruthy();
    expect(screen.getByText('Codex thread：thread-codex-3')).toBeTruthy();
    expect(screen.getByText('Codex turn：尚未获得')).toBeTruthy();
    expect(screen.getByText('Codex final text')).toBeTruthy();
    expect(screen.getByText('RELAY_E2E_OK')).toBeTruthy();
    expect(screen.getByText('Outbound ChatGPT message：chatgpt-message-18')).toBeTruthy();
    expect(screen.getByText(`阻塞原因：${completedCycle.blockReason}`)).toBeTruthy();
  });

  it('以后台顺序显示 cycle，并为失败 cycle 显示错误而不显示成功状态', () => {
    const failedCycle: RelayCodexCycle = {
      ...completedCycle,
      id: 'cycle-2',
      cycleNumber: 2,
      status: 'FAILED',
      resultText: null,
      outboundChatgptMessageId: null,
      errorText: 'Codex turn 启动失败。',
      blockReason: null,
    };
    render(<CodexCommunicationPanel cycles={[completedCycle, failedCycle]} />);

    expect(screen.getAllByRole('article').map((card) => card.textContent)).toEqual([
      expect.stringContaining('Cycle 3'),
      expect.stringContaining('Cycle 2'),
    ]);
    expect(screen.getByText('Cycle 2 · 失败')).toBeTruthy();
    expect(screen.getByText('错误：Codex turn 启动失败。')).toBeTruthy();
    expect(screen.queryByText('Cycle 2 · Codex 已完成')).toBeNull();
  });

  it('显示加载和空 cycle 状态', () => {
    const { rerender } = render(<CodexCommunicationPanel cycles={null} />);
    expect(screen.getByText('正在读取 Codex 通讯状态…')).toBeTruthy();

    rerender(<CodexCommunicationPanel cycles={[]} />);
    expect(screen.getByText('尚未开始 Codex 循环。')).toBeTruthy();
  });
});
