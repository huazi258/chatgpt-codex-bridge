export type RelayCodexThreadStatus = 'idle' | 'notLoaded' | 'active' | 'systemError' | string;

export interface RelayCodexThreadCandidate {
  threadId: string;
  name?: string | null;
  source: string;
  status: RelayCodexThreadStatus;
  branch?: string | null;
  recencyAt?: string | null;
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
