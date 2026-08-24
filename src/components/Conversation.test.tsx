import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { Conversation } from './Conversation';

describe('Conversation timeline', () => {
  it('按 relay message sequence 展示混合流程，并且不重复 Codex prompt/result', () => {
    const { container } = render(<Conversation
      session={{ id: 'session', name: '会话', workingDirectory: 'G:\\project', maxCycles: 12, maxRuntimeMinutes: 240, retryTemplate: 'retry', phase: 'WAITING_FOR_CHATGPT', stopAfterTurn: false, invalidReplyCount: 0, startedCycles: 1 }}
      messages={[
        { id: 'reply-2', sequenceNumber: 5, direction: 'FROM_CHATGPT', kind: 'AUTOMATION', text: 'ChatGPT 后续回复', deliveryState: 'DELIVERED' },
        { id: 'forward', sequenceNumber: 4, direction: 'TO_CHATGPT', kind: 'AUTOMATION', text: 'Codex 最终结果', deliveryState: 'DELIVERED' },
        { id: 'codex-result', sequenceNumber: 3, direction: 'FROM_CODEX', kind: 'SYSTEM', text: 'Codex 最终结果', deliveryState: 'DELIVERED' },
        { id: 'prompt', sequenceNumber: 2, direction: 'TO_CODEX', kind: 'AUTOMATION', text: '实现时间线', deliveryState: 'DELIVERED' },
        { id: 'reply-1', sequenceNumber: 1, direction: 'FROM_CHATGPT', kind: 'AUTOMATION', text: 'ChatGPT 自动化回复', deliveryState: 'DELIVERED' },
      ]}
      cycles={[{ id: 'cycle-1', moduleId: 'session', cycleNumber: 1, status: 'CODEX_COMPLETED', promptText: '实现时间线', resultText: 'Codex 最终结果', createdAt: '', updatedAt: '' }]}
      recoveryMessages={[]}
      notice="已就绪"
      text=""
      kind="MANUAL"
      busy={false}
      onTextChange={vi.fn()}
      onKindChange={vi.fn()}
      onSend={vi.fn()}
      onRetryUnknown={vi.fn()}
      onContinueUnknown={vi.fn()}
    />);

    const transcript = container.textContent ?? '';
    expect(transcript.indexOf('ChatGPT 自动化回复')).toBeLessThan(transcript.indexOf('实现时间线'));
    expect(transcript.indexOf('实现时间线')).toBeLessThan(transcript.indexOf('Codex 最终结果'));
    expect(transcript.indexOf('Codex 最终结果')).toBeLessThan(transcript.indexOf('Codex 结果已回传 ChatGPT'));
    expect(transcript.indexOf('Codex 结果已回传 ChatGPT')).toBeLessThan(transcript.indexOf('ChatGPT 后续回复'));
    expect(screen.getAllByText('实现时间线')).toHaveLength(1);
    expect(screen.getAllByText('Codex 最终结果')).toHaveLength(1);
  });
});
