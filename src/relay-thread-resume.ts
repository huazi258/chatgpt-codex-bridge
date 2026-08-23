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
