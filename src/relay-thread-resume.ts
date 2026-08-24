export type RelayCodexThreadStatus = 'idle' | 'notLoaded' | 'active' | 'systemError' | string;

export interface RelayCodexThreadCandidate {
  threadId: string;
  name?: string | null;
  source: string;
  status: RelayCodexThreadStatus;
  branch?: string | null;
  recencyAt: number | null;
  selectable: boolean;
  disabledReason?: string | null;
}

export type RelayCodexThreadTarget =
  | { mode: 'NEW' }
  | { mode: 'EXISTING'; threadId: string };

export interface RelayModuleCreationInput {
  name: string;
  workingDirectory: string;
  maxCycles: number;
  maxRuntimeMinutes: number;
  retryTemplate: string;
  codexThreadTarget: RelayCodexThreadTarget;
}

export interface RelayThreadResumeModuleFields {
  resumeThreadId?: string | null;
  codexRecoveryReason?: string | null;
}

export type RelayCodexRecoveryAction =
  | { type: 'RETRY_RESUME' }
  | { type: 'REACQUIRE_THREAD' }
  | { type: 'START_NEW_THREAD' }
  | { type: 'RETRY_TURN_START' }
  | { type: 'SELECT_EXISTING_THREAD'; threadId: string };

export type RelayCodexRecoveryAllowedAction =
  | { type: 'RETRY_RESUME' }
  | { type: 'REACQUIRE_THREAD' }
  | { type: 'START_NEW_THREAD' }
  | { type: 'RETRY_TURN_START' }
  | { type: 'SELECT_EXISTING_THREAD' };

export interface RelayCodexThreadRegistryState {
  threadId: string;
  workingDirectory: string;
  state: 'RESERVED' | 'ACTIVE' | 'RELEASED' | 'UNAVAILABLE' | string;
  ownerModuleId?: string | null;
  lastModuleId?: string | null;
  reservationPreviousState?: string | null;
  updatedAt: string;
}

export interface RelayCodexThreadRecoveryCycle {
  id: string;
  cycleNumber: number;
  status: string;
  promptText: string;
}

export interface RelayCodexThreadStateSnapshot {
  moduleId: string;
  workingDirectory: string;
  intendedThreadId?: string | null;
  acquiredThreadId?: string | null;
  registry?: RelayCodexThreadRegistryState | null;
  recoveryReason?: string | null;
  pendingCycle?: RelayCodexThreadRecoveryCycle | null;
  summary: string;
  allowedActions: RelayCodexRecoveryAllowedAction[];
}
