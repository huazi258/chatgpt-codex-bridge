export type CodexCycleStatus =
  | 'WAITING_TO_SEND_CODEX'
  | 'CODEX_RUNNING'
  | 'CODEX_COMPLETED'
  | 'WAITING_FOR_CHATGPT'
  | 'SENDING_TO_CHATGPT'
  | 'DELIVERED_TO_CHATGPT'
  | 'FAILED';

export interface RelayCodexCycle {
  id: string;
  moduleId: string;
  cycleNumber: number;
  status: CodexCycleStatus;
  promptText: string;
  codexThreadId?: string | null;
  codexTurnId?: string | null;
  resultText?: string | null;
  outboundChatgptMessageId?: string | null;
  errorText?: string | null;
  createdAt: string;
  codexStartedAt?: string | null;
  codexCompletedAt?: string | null;
  relayQueuedAt?: string | null;
  relayDeliveredAt?: string | null;
  updatedAt: string;
  blockReason?: string | null;
}

export interface RelayChannelSnapshot {
  chatgpt: {
    status: 'IDLE' | 'IN_FLIGHT' | 'RECOVERY_BLOCKED';
    activeModuleId?: string | null;
    activeModuleName?: string | null;
    activeMessageId?: string | null;
    activeKind?: 'MANUAL' | 'AUTOMATION' | 'SYSTEM' | null;
    activePhase?: string | null;
    recoveryBlockerCount: number;
  };
  codex: {
    status: 'IDLE' | 'RUNNING';
    activeModuleId?: string | null;
    activeModuleName?: string | null;
    cycleNumber?: number | null;
    codexThreadId?: string | null;
    codexTurnId?: string | null;
    cycleStatus?: CodexCycleStatus | null;
  };
}

export function codexCycleStatusLabel(status: CodexCycleStatus): string {
  switch (status) {
    case 'WAITING_TO_SEND_CODEX': return '等待发送 Codex';
    case 'CODEX_RUNNING': return 'Codex 运行中';
    case 'CODEX_COMPLETED': return 'Codex 已完成';
    case 'WAITING_FOR_CHATGPT': return '等待回传 ChatGPT';
    case 'SENDING_TO_CHATGPT': return '回传 ChatGPT 中';
    case 'DELIVERED_TO_CHATGPT': return '回传完成';
    case 'FAILED': return '失败';
  }
}
