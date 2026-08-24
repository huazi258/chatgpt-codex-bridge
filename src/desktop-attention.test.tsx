import { describe, expect, it, vi } from 'vitest';
import { AttentionTransitionTracker, DesktopAttentionNotifier, attentionNotice, type DesktopAttentionDependencies } from './desktop-attention';

function dependencies(focused: boolean, granted = true): DesktopAttentionDependencies {
  return { isFocused: vi.fn(async () => focused), requestUserAttention: vi.fn(async () => undefined), isPermissionGranted: vi.fn(async () => granted), requestPermission: vi.fn(async () => granted ? 'granted' : 'denied'), sendNotification: vi.fn() };
}

describe('desktop attention', () => {
  it('只为进入人工关注状态的 session 通知一次，离开后允许再次通知', () => {
    const tracker = new AttentionTransitionTracker();
    const blocked = { id: 'one', name: 'Bridge UI', phase: 'BLOCKED' };
    expect(tracker.entered([blocked])).toEqual([blocked]);
    expect(tracker.entered([blocked])).toEqual([]);
    expect(tracker.entered([{ ...blocked, phase: 'WAITING_FOR_CHATGPT' }])).toEqual([]);
    expect(tracker.entered([blocked])).toEqual([blocked]);
  });

  it('focused 时不发 toast，unfocused 时请求 informational attention 并发送通知', async () => {
    const notifier = new DesktopAttentionNotifier();
    const focused = dependencies(true);
    await notifier.notify(attentionNotice({ id: 'one', name: 'Bridge UI', phase: 'BLOCKED' }), focused);
    expect(focused.sendNotification).not.toHaveBeenCalled();
    const unfocused = dependencies(false);
    await notifier.notify(attentionNotice({ id: 'one', name: 'Bridge UI', phase: 'BLOCKED' }), unfocused);
    expect(unfocused.requestUserAttention).toHaveBeenCalledWith(2);
    expect(unfocused.sendNotification).toHaveBeenCalledWith(expect.objectContaining({ title: 'Bridge UI 需要人工处理' }));
  });

  it('notification permission 被拒绝时仍请求 taskbar attention 且不抛出', async () => {
    const notifier = new DesktopAttentionNotifier();
    const denied = dependencies(false, false);
    await notifier.notify(attentionNotice({ id: 'one', name: 'Bridge UI', phase: 'FAILED' }), denied);
    expect(denied.requestUserAttention).toHaveBeenCalledWith(2);
    expect(denied.sendNotification).not.toHaveBeenCalled();
  });
});
