export type RelayCodexInputStatus = 'PENDING' | 'ANSWERING' | 'ANSWERED' | 'INTERRUPTED' | 'EXPIRED';

export interface RelayCodexInputQuestion {
  id: string; header: string; question: string;
  options?: Array<{ label: string; description: string }> | null;
  isOther?: boolean; isSecret?: boolean;
}

export interface RelayCodexInputRequest {
  id: string; cycleId: string; cycleNumber?: number; codexThreadId: string; codexTurnId: string;
  questionsJson: string; answersJson?: string | null; secretAnswerStatusJson: string;
  status: RelayCodexInputStatus; errorText?: string | null;
}

export interface RelayCodexInputAnswerInput { questionId: string; answer: string; }
