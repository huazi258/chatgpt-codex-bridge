import type { RelayCodexCycle } from './relay-observability';
import type { RelayThreadResumeModuleFields } from './relay-thread-resume';

export type RelayKind = 'MANUAL' | 'AUTOMATION';

export interface RelayModule extends RelayThreadResumeModuleFields {
  id: string;
  name: string;
  workingDirectory: string;
  maxCycles: number;
  maxRuntimeMinutes: number;
  retryTemplate: string;
  phase: string;
  invalidReplyCount: number;
  startedCycles: number;
  stopAfterTurn: boolean;
}

export interface RelayMessage {
  id: string;
  sequenceNumber: number;
  direction: 'TO_CHATGPT' | 'FROM_CHATGPT' | 'TO_CODEX' | 'FROM_CODEX';
  kind: 'MANUAL' | 'AUTOMATION' | 'SYSTEM';
  text: string;
  deliveryState: string;
}

export interface RelayRecoveryMessage {
  messageId: string;
  moduleId: string;
  moduleName: string;
  sequenceNumber: number;
  kind: RelayKind;
  createdAt: string;
}

export interface PairingInfo {
  endpoint: string;
  pairingSecret: string;
  paired: boolean;
}

export interface BridgeStatus {
  phase: string;
  detail: string;
}

export const terminalPhases = new Set(['COMPLETED', 'STOPPED']);

export function phaseLabel(phase: string): string {
  switch (phase) {
    case 'CODEX_RUNNING': return '运行中';
    case 'CODEX_STARTING': return '等待 Codex';
    case 'WAITING_FOR_CHATGPT':
    case 'SENDING_TO_CHATGPT': return '等待 ChatGPT';
    case 'WAITING_FOR_ACCEPTANCE': return '等待验收';
    case 'COMPLETED': return '已完成';
    case 'BLOCKED':
    case 'RECOVERY_REQUIRED': return '已阻塞';
    case 'STOPPED': return '已停止';
    case 'FAILED': return '失败';
    case 'READY': return '就绪';
    default: return phase;
  }
}

export function shortId(value?: string | null): string {
  if (!value) return '—';
  return value.length > 18 ? `${value.slice(0, 9)}…${value.slice(-6)}` : value;
}

export function currentCycle(cycles: RelayCodexCycle[]): RelayCodexCycle | null {
  return cycles.find((cycle) => cycle.status === 'CODEX_RUNNING') ?? cycles[0] ?? null;
}
