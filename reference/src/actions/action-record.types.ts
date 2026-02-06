export type RiskLevel = "low" | "medium" | "high" | "critical";

export type ActionOutcome = "success" | "failed" | "blocked" | "skipped";

export type ActionFailureReason =
  | "guardrail_denied"
  | "execution_error"
  | "timeout"
  | "user_aborted"
  | "unknown";

export type ActionRecord = {
  id: string;
  sessionKey: string;
  timestamp: string; // ISO timestamp
  tool: string;
  summary: string;
  planned: boolean;
  outcome?: ActionOutcome;
  failureReason?: ActionFailureReason;
  riskLevel: RiskLevel;
  /**
   * Optional metadata used for lightweight reporting (files touched, sites visited).
   * Keep this sparse; Sprint 14 is a summary layer, not a full trace dump.
   */
  meta?: {
    filePath?: string;
    url?: string;
  };
};

export type ListActionsOpts = {
  sessionKey?: string;
  onlyExecuted?: boolean;
  limit?: number;
};
