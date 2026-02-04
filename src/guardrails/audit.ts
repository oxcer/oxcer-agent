/**
 * OXCER: Append-only audit log for guardrail decisions and high-impact tool actions.
 * Designed to support enterprise compliance and incident forensics. Write failures
 * are non-fatal so audit log (e.g. disk full) never brings down the agent.
 */

import fs from "node:fs";
import path from "node:path";
import type { PolicyDecision } from "./orchestrator.js";
import type { GuardrailMode } from "../config/guardrails.js";
import type { ResolvedGuardrailsAuditConfig } from "../config/guardrails.js";
import { emitGuardrailMetric } from "./metrics.js";

const EVENT_TYPES = {
  action: "guardrail.action",
  result: "guardrail.result",
  nodeInvoke: "guardrail.node.invoke",
  nodeInvokeComplete: "guardrail.node.invoke.complete",
  humanReview: "guardrail.human.review",
} as const;

let auditConfig: ResolvedGuardrailsAuditConfig | null = null;

/** Set resolved audit config (e.g. at gateway/agent startup). If not set, audit writes are skipped. */
export function setGuardrailAuditConfig(config: ResolvedGuardrailsAuditConfig | null): void {
  auditConfig = config;
}

export function getGuardrailAuditConfig(): ResolvedGuardrailsAuditConfig | null {
  return auditConfig;
}

function timestampRfc3339(ms: number = Date.now()): string {
  return new Date(ms).toISOString();
}

function ensureAuditDir(filePath: string): void {
  const dir = path.dirname(filePath);
  try {
    fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
  } catch (err) {
    console.debug(`[guardrail:audit] mkdir failed (non-fatal): ${(err as Error)?.message ?? err}`);
  }
}

/**
 * Append one JSON line. Never throws; logs and continues on failure.
 * TODO: optional maxSizeBytes / rotation (e.g. actions.jsonl.1, .2) in a future sprint.
 */
function appendJsonLine(filePath: string, obj: Record<string, unknown>): void {
  const config = getGuardrailAuditConfig();
  if (!config?.enabled) {
    return;
  }
  ensureAuditDir(filePath);
  const line = JSON.stringify(obj) + "\n";
  try {
    fs.appendFileSync(filePath, line, { flag: "a" });
  } catch (err) {
    console.debug(`[guardrail:audit] append failed (non-fatal): ${(err as Error)?.message ?? err}`);
    return;
  }
  emitGuardrailMetric(obj);
}

export type GuardrailActionAuditEntry = {
  actionId: string;
  tool: string;
  args: Record<string, unknown>;
  summary?: string;
  riskLevel?: string;
  sessionKey?: string;
  agentId?: string;
  channel?: string;
  timestamp: number;
  decision: PolicyDecision["decision"];
  reason?: string;
  mode: GuardrailMode;
  failClosed: boolean;
  category?: "action";
  errorMessage?: string;
};

export type GuardrailResultAuditEntry = {
  tool: string;
  args: Record<string, unknown>;
  decision: PolicyDecision["decision"];
  reason?: string;
  mode: GuardrailMode;
  failClosed: boolean;
  redacted: boolean;
  timestamp: number;
  sessionKey?: string;
  agentId?: string;
  category?: "result";
  errorMessage?: string;
};

export type GuardrailNodeInvokeAuditEntry = {
  nodeId: string;
  command: string;
  params?: unknown;
  decision?: PolicyDecision["decision"];
  reason?: string;
  proceed: boolean;
  forceApproval?: boolean;
  timestamp: number;
  category?: "node.invoke";
  errorMessage?: string;
};

export type GuardrailNodeInvokeCompleteEntry = {
  nodeId: string;
  command: string;
  success: boolean;
  timestamp: number;
  errorMessage?: string;
};

/** Unified event shape for JSONL and metrics. eventType discriminates. */
export type GuardrailAuditEvent = {
  eventId: string;
  eventType: (typeof EVENT_TYPES)[keyof typeof EVENT_TYPES];
  timestamp: string;
} & Record<string, unknown>;

function uuid(): string {
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    const v = c === "x" ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
}

/** Called after runActionGuardrail has a decision. Writes to JSONL when audit enabled. */
export function recordGuardrailActionDecision(entry: GuardrailActionAuditEntry): void {
  const config = getGuardrailAuditConfig();
  if (config?.devConsole && process.env.NODE_ENV === "development") {
    console.debug("[guardrail:action]", entry.actionId, entry.tool, entry.decision, entry.mode);
  }
  if (!config?.enabled) {
    return;
  }
  const event: GuardrailAuditEvent = {
    eventId: uuid(),
    eventType: EVENT_TYPES.action,
    timestamp: timestampRfc3339(entry.timestamp),
    actionId: entry.actionId,
    tool: entry.tool,
    args: entry.args,
    summary: entry.summary,
    riskLevel: entry.riskLevel,
    sessionKey: entry.sessionKey,
    agentId: entry.agentId,
    channel: entry.channel,
    decision: entry.decision,
    reason: entry.reason,
    mode: entry.mode,
    failClosed: entry.failClosed,
    category: entry.category,
    errorMessage: entry.errorMessage,
  };
  appendJsonLine(config.filePath, event);
}

/** Called after runResultGuardrail decides allow/deny/redact. */
export function recordGuardrailResultDecision(entry: GuardrailResultAuditEntry): void {
  const config = getGuardrailAuditConfig();
  if (config?.devConsole && process.env.NODE_ENV === "development") {
    console.debug("[guardrail:result]", entry.tool, entry.decision, entry.redacted, entry.mode);
  }
  if (!config?.enabled) {
    return;
  }
  const event: GuardrailAuditEvent = {
    eventId: uuid(),
    eventType: EVENT_TYPES.result,
    timestamp: timestampRfc3339(entry.timestamp),
    tool: entry.tool,
    args: entry.args,
    decision: entry.decision,
    reason: entry.reason,
    mode: entry.mode,
    failClosed: entry.failClosed,
    redacted: entry.redacted,
    sessionKey: entry.sessionKey,
    agentId: entry.agentId,
    category: entry.category,
    errorMessage: entry.errorMessage,
  };
  appendJsonLine(config.filePath, event);
}

/** Called when a guarded node.invoke decision is made. */
export function recordGuardrailNodeInvoke(entry: GuardrailNodeInvokeAuditEntry): void {
  const config = getGuardrailAuditConfig();
  if (config?.devConsole && process.env.NODE_ENV === "development") {
    console.debug(
      "[guardrail:node.invoke]",
      entry.nodeId,
      entry.command,
      entry.proceed,
      entry.decision,
    );
  }
  if (!config?.enabled) {
    return;
  }
  const event: GuardrailAuditEvent = {
    eventId: uuid(),
    eventType: EVENT_TYPES.nodeInvoke,
    timestamp: timestampRfc3339(entry.timestamp),
    nodeId: entry.nodeId,
    command: entry.command,
    params: entry.params,
    decision: entry.decision,
    reason: entry.reason,
    proceed: entry.proceed,
    forceApproval: entry.forceApproval,
    category: entry.category,
    errorMessage: entry.errorMessage,
  };
  appendJsonLine(config.filePath, event);
}

/** Called after a guarded node.invoke has completed (success or failure). */
export function recordGuardrailNodeInvokeComplete(entry: GuardrailNodeInvokeCompleteEntry): void {
  const config = getGuardrailAuditConfig();
  if (config?.devConsole && process.env.NODE_ENV === "development") {
    console.debug("[guardrail:node.invoke.complete]", entry.nodeId, entry.command, entry.success);
  }
  if (!config?.enabled) {
    return;
  }
  const event: GuardrailAuditEvent = {
    eventId: uuid(),
    eventType: EVENT_TYPES.nodeInvokeComplete,
    timestamp: timestampRfc3339(entry.timestamp),
    nodeId: entry.nodeId,
    command: entry.command,
    success: entry.success,
    errorMessage: entry.errorMessage,
  };
  appendJsonLine(config.filePath, event);
}

export type GuardrailHumanReviewAuditEntry = {
  eventId: string;
  originalDecision: PolicyDecision["decision"];
  humanDecision: "approve" | "reject";
  reviewer: string;
  note?: string;
  timestamp: number;
  tool: string;
  summary?: string;
  riskLevel?: string;
};

/** Called when a human submits a review for a needs_human guardrail event. */
export function recordGuardrailHumanReview(entry: GuardrailHumanReviewAuditEntry): void {
  const config = getGuardrailAuditConfig();
  if (config?.devConsole && process.env.NODE_ENV === "development") {
    console.debug("[guardrail:human-review]", entry.eventId, entry.humanDecision, entry.reviewer);
  }
  if (!config?.enabled) {
    return;
  }
  const event: GuardrailAuditEvent = {
    eventId: uuid(),
    eventType: "guardrail.human.review" as const,
    timestamp: timestampRfc3339(entry.timestamp),
    reviewedEventId: entry.eventId,
    originalDecision: entry.originalDecision,
    humanDecision: entry.humanDecision,
    reviewer: entry.reviewer,
    note: entry.note,
    tool: entry.tool,
    summary: entry.summary,
    riskLevel: entry.riskLevel,
    label: entry.humanDecision === "approve" ? "human_approved" : "human_rejected",
  };
  appendJsonLine(config.filePath, event);
}
