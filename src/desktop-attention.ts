import { getCurrentWindow, UserAttentionType } from '@tauri-apps/api/window';
import { isPermissionGranted, requestPermission, sendNotification } from '@tauri-apps/plugin-notification';

export type AttentionPhase = 'BLOCKED' | 'WAITING_FOR_ACCEPTANCE' | 'RECOVERY_REQUIRED' | 'FAILED';

export interface AttentionSession {
  id: string;
  name: string;
  phase: string;
}

export interface AttentionNotice {
  title: string;
  body: string;
}

const attentionPhases = new Set<AttentionPhase>(['BLOCKED', 'WAITING_FOR_ACCEPTANCE', 'RECOVERY_REQUIRED', 'FAILED']);

export class AttentionTransitionTracker {
  private active = new Set<string>();

  entered(sessions: AttentionSession[]): AttentionSession[] {
    const next = new Set(sessions.filter((session) => attentionPhases.has(session.phase as AttentionPhase)).map((session) => `${session.id}:${session.phase}`));
    const entered = sessions.filter((session) => next.has(`${session.id}:${session.phase}`) && !this.active.has(`${session.id}:${session.phase}`));
    this.active = next;
    return entered;
  }
}

export function attentionNotice(session: AttentionSession, detail?: string | null): AttentionNotice {
  switch (session.phase) {
    case 'BLOCKED': return { title: `${session.name} 需要人工处理`, body: detail || 'ChatGPT 暂停了自动流程，等待你的输入。' };
    case 'WAITING_FOR_ACCEPTANCE': return { title: `${session.name} 等待人工验收`, body: 'ChatGPT 已请求结束当前会话。' };
    case 'RECOVERY_REQUIRED': return { title: `${session.name} 需要恢复处理`, body: detail || '会话存在需要人工确认的恢复状态。' };
    default: return { title: `${session.name} 运行失败`, body: detail || '会话运行失败，请打开应用查看。' };
  }
}

export interface DesktopAttentionDependencies {
  isFocused: () => Promise<boolean>;
  requestUserAttention: (type: UserAttentionType | null) => Promise<void>;
  isPermissionGranted: () => Promise<boolean>;
  requestPermission: () => Promise<NotificationPermission>;
  sendNotification: (notice: AttentionNotice) => void;
}

export function desktopAttentionDependencies(): DesktopAttentionDependencies {
  const window = getCurrentWindow();
  return {
    isFocused: () => window.isFocused(),
    requestUserAttention: (type) => window.requestUserAttention(type),
    isPermissionGranted,
    requestPermission,
    sendNotification,
  };
}

export class DesktopAttentionNotifier {
  private permissionRequested = false;

  async notify(notice: AttentionNotice, dependencies?: DesktopAttentionDependencies): Promise<void> {
    try {
      const desktop = dependencies ?? desktopAttentionDependencies();
      if (await desktop.isFocused()) {
        await desktop.requestUserAttention(null);
        return;
      }
      await desktop.requestUserAttention(UserAttentionType.Informational);
      let granted = await desktop.isPermissionGranted();
      if (!granted && !this.permissionRequested) {
        this.permissionRequested = true;
        granted = (await desktop.requestPermission()) === 'granted';
      }
      if (granted) desktop.sendNotification(notice);
    } catch {
      // Notifications are optional desktop feedback and must not affect relay state.
    }
  }

  async clearAttention(dependencies?: DesktopAttentionDependencies): Promise<void> {
    try { await (dependencies ?? desktopAttentionDependencies()).requestUserAttention(null); } catch { /* best effort */ }
  }
}
