export type ModuleStatus = 'INACTIVE';

export interface Budget {
  maxRounds: number;
  moduleTimeoutMinutes: number;
  globalTimeoutMinutes: number;
}

export interface ModuleRecord {
  id: string;
  name: string;
  repositoryPath: string;
  targetBranch: string;
  chatgptTabId: number;
  status: ModuleStatus;
  budget: Budget;
  createdAt: string;
  updatedAt: string;
}

export type CodexExecutionPhase = 'STARTING' | 'RUNNING' | 'COMPLETED' | 'BLOCKED' | 'FAILED';

export interface CodexExecutionEvent {
  moduleId: string;
  phase: CodexExecutionPhase;
  statusLine: string;
  threadId?: string;
  turnId?: string;
}

export interface CodexTurnResult {
  moduleId: string;
  status: CodexExecutionPhase;
  summary: string;
  threadId?: string;
  turnId?: string;
}

export interface ChatGptPairingInfo {
  endpoint: string;
  pairingSecret: string;
  paired: boolean;
  boundTabId?: number;
}

export interface ChatGptBridgeStatus {
  phase: string;
  detail: string;
  tabId?: number;
  protocolState?: string;
}

export interface ModuleDraft {
  name: string;
  repositoryPath: string;
  targetBranch: string;
  chatgptTabId: string;
  maxRounds: string;
  moduleTimeoutMinutes: string;
  globalTimeoutMinutes: string;
}

export const emptyDraft: ModuleDraft = {
  name: '',
  repositoryPath: '',
  targetBranch: '',
  chatgptTabId: '',
  maxRounds: '6',
  moduleTimeoutMinutes: '120',
  globalTimeoutMinutes: '240'
};
