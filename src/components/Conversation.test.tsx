import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { Conversation } from './Conversation';

const baseProps = {
  session: { id: 'session', name: '会话', workingDirectory: 'G:\\project', maxCycles: 12, maxRuntimeMinutes: 240, retryTemplate: 'retry', phase: 'WAITING_FOR_CHATGPT', stopAfterTurn: false, invalidReplyCount: 0, startedCycles: 1 },
  recoveryMessages: [],
  notice: '已就绪',
  text: '',
  kind: 'MANUAL' as const,
  busy: false,
  onTextChange: vi.fn(),
  onKindChange: vi.fn(),
  onSend: vi.fn(),
  onRetryUnknown: vi.fn(),
  onContinueUnknown: vi.fn(),
};

describe('Conversation timeline', () => {
  it('按消息顺序将 completed 状态放在 Codex result 后，并使用 outbound ID 识别回传', () => {
    const { container } = render(<Conversation
      {...baseProps}
      messages={[
        { id: 'reply-2', sequenceNumber: 5, direction: 'FROM_CHATGPT', kind: 'AUTOMATION', text: 'ChatGPT 后续回复', deliveryState: 'DELIVERED' },
        { id: 'outbound-1', sequenceNumber: 4, direction: 'TO_CHATGPT', kind: 'AUTOMATION', text: 'Codex 最终结果', deliveryState: 'DELIVERED' },
        { id: 'codex-result', sequenceNumber: 3, direction: 'FROM_CODEX', kind: 'SYSTEM', text: 'Codex 最终结果', deliveryState: 'DELIVERED' },
        { id: 'prompt', sequenceNumber: 2, direction: 'TO_CODEX', kind: 'AUTOMATION', text: '实现时间线', deliveryState: 'DELIVERED' },
        { id: 'reply-1', sequenceNumber: 1, direction: 'FROM_CHATGPT', kind: 'AUTOMATION', text: 'ChatGPT 自动化回复', deliveryState: 'DELIVERED' },
      ]}
      cycles={[{ id: 'cycle-1', moduleId: 'session', cycleNumber: 1, status: 'CODEX_COMPLETED', promptText: '实现时间线', resultText: 'Codex 最终结果', outboundChatgptMessageId: 'outbound-1', createdAt: '', updatedAt: '' }]}
    />);

    const transcript = container.textContent ?? '';
    expect(transcript.indexOf('第 1 轮 · Cycle 1')).toBeLessThan(transcript.indexOf('实现时间线'));
    expect(transcript.indexOf('实现时间线')).toBeLessThan(transcript.indexOf('Codex 最终结果'));
    expect(transcript.indexOf('Codex 最终结果')).toBeLessThan(transcript.indexOf('✓ Codex 已完成本轮任务'));
    expect(transcript.indexOf('✓ Codex 已完成本轮任务')).toBeLessThan(transcript.indexOf('Codex 结果已回传 ChatGPT'));
    expect(transcript.indexOf('Codex 结果已回传 ChatGPT')).toBeLessThan(transcript.indexOf('ChatGPT 后续回复'));
    expect(screen.getAllByText('实现时间线')).toHaveLength(1);
    expect(screen.getAllByText('Codex 最终结果')).toHaveLength(1);
  });

  it('明确关联的 UNKNOWN 回传显示真实状态，而相同文本的普通自动化消息不会被隐藏', () => {
    render(<Conversation
      {...baseProps}
      messages={[
        { id: 'prompt', sequenceNumber: 1, direction: 'TO_CODEX', kind: 'AUTOMATION', text: '执行任务', deliveryState: 'DELIVERED' },
        { id: 'codex-result', sequenceNumber: 2, direction: 'FROM_CODEX', kind: 'SYSTEM', text: '相同文本', deliveryState: 'DELIVERED' },
        { id: 'ordinary-automation', sequenceNumber: 3, direction: 'TO_CHATGPT', kind: 'AUTOMATION', text: '相同文本', deliveryState: 'DELIVERED' },
        { id: 'outbound-1', sequenceNumber: 4, direction: 'TO_CHATGPT', kind: 'AUTOMATION', text: '相同文本', deliveryState: 'UNKNOWN' },
      ]}
      cycles={[{ id: 'cycle-1', moduleId: 'session', cycleNumber: 1, status: 'WAITING_FOR_CHATGPT', promptText: '执行任务', resultText: '相同文本', outboundChatgptMessageId: 'outbound-1', createdAt: '', updatedAt: '' }]}
    />);

    expect(screen.getAllByText('相同文本')).toHaveLength(2);
    expect(screen.getByText('Codex 结果回传状态不确定')).toBeTruthy();
  });
});
