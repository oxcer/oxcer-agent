/**
 * OXCER: Session actions + reports controller for UI.
 * Sprint 14: "inspectable run" summaries (actions list, pre/post reports).
 */

import type { Gateway } from "../gateway.js";

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
  timestamp: string;
  tool: string;
  summary: string;
  planned: boolean;
  outcome?: ActionOutcome;
  failureReason?: ActionFailureReason;
  riskLevel: RiskLevel;
  meta?: { filePath?: string; url?: string };
};

export type PreExecutionHighlight = {
  actionId: string;
  riskLevel: RiskLevel;
  reason: string;
};

export type PreExecutionSummary = {
  sessionKey: string;
  plannedActions: ActionRecord[];
  highlights: PreExecutionHighlight[];
};

export type PostExecutionSummary = {
  sessionKey: string;
  plannedActions: ActionRecord[];
  executedActions: ActionRecord[];
  blockedActions: ActionRecord[];
  filesTouched: string[];
  sitesVisited: string[];
};

export async function listSessionActions(
  gateway: Gateway,
  params: { sessionKey: string; onlyExecuted?: boolean; limit?: number },
): Promise<ActionRecord[]> {
  const result = await gateway.request("session.actions.list", params);
  if (!result.ok || !result.payload) {
    return [];
  }
  const payload = result.payload as { actions?: ActionRecord[] };
  return payload.actions ?? [];
}

export async function getPreExecutionReport(
  gateway: Gateway,
  params: { sessionKey: string },
): Promise<PreExecutionSummary | null> {
  const result = await gateway.request("session.report.preExecution", params);
  if (!result.ok || !result.payload) {
    return null;
  }
  return result.payload as PreExecutionSummary;
}

export async function getPostExecutionReport(
  gateway: Gateway,
  params: { sessionKey: string },
): Promise<PostExecutionSummary | null> {
  const result = await gateway.request("session.report.postExecution", params);
  if (!result.ok || !result.payload) {
    return null;
  }
  return result.payload as PostExecutionSummary;
}

