import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';

let modules = [
  { id: 'existing', name: '原有模块', workingDirectory: 'G:\\projects\\existing', maxCycles: 12, maxRuntimeMinutes: 240, retryTemplate: '重试', phase: 'READY', invalidReplyCount: 0, startedCycles: 0 },
];

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

invoke.mockImplementation(async (command: string, args?: { input?: { name: string; workingDirectory: string } }) => {
  if (command === 'list_relay_modules') return modules;
  if (command === 'get_chatgpt_pairing') return { endpoint: 'ws://127.0.0.1:17384', pairingSecret: 'secret', paired: false };
  if (command === 'list_relay_messages') return [];
  if (command === 'create_relay_module') {
    const input = args?.input;
    if (!input) throw new Error('missing module input');
    const created = { id: 'created', name: input.name, workingDirectory: input.workingDirectory, maxCycles: 12, maxRuntimeMinutes: 240, retryTemplate: '重试', phase: 'READY', invalidReplyCount: 0, startedCycles: 0 };
    modules = [created, ...modules];
    return created;
  }
  throw new Error(`unexpected command: ${command}`);
});

vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => undefined) }));

describe('传话模块创建入口', () => {
  beforeEach(() => {
    modules = [
      { id: 'existing', name: '原有模块', workingDirectory: 'G:\\projects\\existing', maxCycles: 12, maxRuntimeMinutes: 240, retryTemplate: '重试', phase: 'READY', invalidReplyCount: 0, startedCycles: 0 },
    ];
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
});
