import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';

let modules = [{ id: 'one', name: 'Bridge UI', workingDirectory: 'G:\\project', maxCycles: 12, maxRuntimeMinutes: 240, retryTemplate: 'retry', phase: 'READY', stopAfterTurn: false, invalidReplyCount: 0, startedCycles: 0 }];
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
invoke.mockImplementation(async (command: string) => {
  if (command === 'list_relay_modules') return modules;
  if (command === 'get_chatgpt_pairing') return { endpoint: 'ws://127.0.0.1:8765', pairingSecret: 'secret', paired: true };
  if (command === 'list_relay_messages' || command === 'list_relay_codex_cycles' || command === 'list_relay_recovery_messages') return [];
  if (command === 'get_relay_channel_snapshot') return { chatgpt: { status: 'IDLE', recoveryBlockerCount: 0 }, codex: { status: 'IDLE' } };
  if (command === 'delete_relay_module') { modules = []; return; }
  return undefined;
});
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => undefined) }));

describe('会话工作台', () => {
  afterEach(cleanup);
  beforeEach(() => { modules = [{ id: 'one', name: 'Bridge UI', workingDirectory: 'G:\\project', maxCycles: 12, maxRuntimeMinutes: 240, retryTemplate: 'retry', phase: 'READY', stopAfterTurn: false, invalidReplyCount: 0, startedCycles: 0 }]; invoke.mockClear(); localStorage.clear(); });
  it('展示会话、通道和紧凑 composer', async () => { render(<App />); expect((await screen.findAllByText('Bridge UI')).length).toBeGreaterThan(0); expect(screen.getByText('会话 Sessions')).toBeTruthy(); expect(screen.getByRole('button', { name: /ChatGPT/ })).toBeTruthy(); expect(screen.getByPlaceholderText('输入消息…')).toBeTruthy(); });
  it('等待验收 modal 可关闭并从顶栏重新打开', async () => { modules = [{ ...modules[0], phase: 'WAITING_FOR_ACCEPTANCE' }]; render(<App />); expect(await screen.findByRole('dialog', { name: '等待人工验收' })).toBeTruthy(); fireEvent.click(screen.getByRole('button', { name: '关闭' })); expect(screen.queryByRole('dialog')).toBeNull(); fireEvent.click(screen.getByRole('button', { name: '等待验收' })); expect(await screen.findByRole('dialog', { name: '等待人工验收' })).toBeTruthy(); });
  it('终止使用确认 modal', async () => { render(<App />); fireEvent.click(await screen.findByTitle('终止当前会话')); expect(screen.getByRole('dialog', { name: '终止当前会话？' })).toBeTruthy(); });
  it('删除会话需确认并调用持久化接口', async () => { render(<App />); fireEvent.click(await screen.findByLabelText('打开“Bridge UI”菜单')); fireEvent.click(screen.getByRole('button', { name: '删除会话' })); expect(screen.getByRole('dialog', { name: '删除“Bridge UI”？' })).toBeTruthy(); fireEvent.click(screen.getByRole('button', { name: '删除会话' })); await waitFor(() => expect(invoke).toHaveBeenCalledWith('delete_relay_module', { moduleId: 'one' })); });
});
