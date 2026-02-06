/**
 * OXCER: Generic guardrail helpers for action (pre-exec) and result (post-exec) checks.
 * Domain-agnostic; call sites supply ProposedAction / tool args and handle deny messages.
 */

import type { ResolvedGuardrailsConfig } from "../config/guardrails.js";
import type { GuardrailOrchestrator, PolicyDecision, ProposedAction } from "./orchestrator.js";
import { recordGuardrailActionDecision, recordGuardrailResultDecision } from "./audit.js";
import { getGuardrailEventsStore } from "./events-store.js";
import { deriveActionMetaFromToolArgs, getActionsStore } from "../actions/actions-store.js";

/** Default text to return when result is redacted by guardrails. */
export const REDACTED_RESULT_TEXT = "[Result redacted by guardrails]";

export type ActionGuardrailResult = {
  forceApproval: boolean;
  decision?: PolicyDecision;
};

/**
 * Run pre-exec action guardrail. Returns forceApproval and optional decision; throws on deny
 * when mode is enforce. Call sites may catch and rethrow with a domain-specific message.
 */
export async function runActionGuardrail(
  gConfig: ResolvedGuardrailsConfig | undefined,
  orchestrator: GuardrailOrchestrator | undefined,
  action: ProposedAction,
): Promise<ActionGuardrailResult> {
  const mode = gConfig?.actionMode ?? "off";
  if (mode === "off" || !orchestrator) {
    return { forceApproval: false };
  }
  // Sprint 14: record the planned action for per-session summaries.
  // We record again as "executed" once a decision is available (allow/deny/needs_human).
  if (action.sessionKey) {
    getActionsStore().recordPlannedAction({
      sessionKey: action.sessionKey,
      tool: action.tool,
      summary: action.summary ?? `${action.tool} action`,
      riskLevel:
        (action.riskLevel as "low" | "medium" | "high" | "critical" | undefined) ?? undefined,
      meta: deriveActionMetaFromToolArgs(action.tool, action.args),
    });
  }
  let decision: PolicyDecision;
  try {
    decision = await orchestrator.checkAction({ action });
  } catch (err) {
    const errorMessage = (err as Error)?.message ?? String(err);
    recordGuardrailActionDecision({
      actionId: action.id,
      tool: action.tool,
      args: action.args,
      summary: action.summary,
      riskLevel: action.riskLevel,
      sessionKey: action.sessionKey,
      agentId: action.agentId,
      channel: action.channel,
      timestamp: action.timestamp,
      decision: "deny",
      reason: errorMessage,
      mode,
      failClosed: gConfig?.failClosed ?? false,
      category: "action",
      errorMessage,
    });
    if (gConfig?.failClosed) {
      throw new Error(`Guardrail check failed (fail-closed): ${errorMessage}`);
    }
    console.warn(`Guardrail checkAction error (continuing): ${errorMessage}`);
    return { forceApproval: false };
  }
  if (action.sessionKey) {
    const outcome =
      decision.decision === "deny"
        ? ("blocked" as const)
        : decision.decision === "needs_human"
          ? ("skipped" as const)
          : ("success" as const);
    const failureReason =
      decision.decision === "deny"
        ? ("guardrail_denied" as const)
        : decision.decision === "needs_human"
          ? ("user_aborted" as const)
          : undefined;
    getActionsStore().recordExecutedAction({
      sessionKey: action.sessionKey,
      tool: action.tool,
      summary: action.summary ?? `${action.tool} action`,
      riskLevel:
        (action.riskLevel as "low" | "medium" | "high" | "critical" | undefined) ?? undefined,
      outcome,
      failureReason,
      meta: deriveActionMetaFromToolArgs(action.tool, action.args),
    });
  }
  recordGuardrailActionDecision({
    actionId: action.id,
    tool: action.tool,
    args: action.args,
    summary: action.summary,
    riskLevel: action.riskLevel,
    sessionKey: action.sessionKey,
    agentId: action.agentId,
    channel: action.channel,
    timestamp: action.timestamp,
    decision: decision.decision,
    reason: "reason" in decision ? decision.reason : undefined,
    mode,
    failClosed: gConfig?.failClosed ?? false,
    category: "action",
  });

  // Sprint 9: Emit event to store for UI surfacing
  if (mode === "warn" || mode === "enforce") {
    const store = getGuardrailEventsStore();
    store.addEvent({
      type: "action",
      decision: decision.decision,
      reason: "reason" in decision ? decision.reason : undefined,
      summary: action.summary ?? `${action.tool} action`,
      tool: action.tool,
      args: action.args,
      riskLevel: action.riskLevel,
      sessionKey: action.sessionKey,
      agentId: action.agentId,
      channel: action.channel,
    });
  }
  if (mode === "warn") {
    console.info(
      `Guardrail action decision: ${decision.decision}${"reason" in decision ? ` — ${decision.reason}` : ""}`,
    );
    return { forceApproval: false, decision };
  }
  if (decision.decision === "deny") {
    throw new Error(decision.reason);
  }
  if (decision.decision === "needs_human") {
    return { forceApproval: true, decision };
  }
  return { forceApproval: false, decision };
}

/**
 * Run post-exec result guardrail. Returns true if the result should be redacted.
 */
export async function runResultGuardrail(
  gConfig: ResolvedGuardrailsConfig | undefined,
  orchestrator: GuardrailOrchestrator | undefined,
  tool: string,
  args: Record<string, unknown>,
  result: unknown,
  context: { sessionKey?: string; agentId?: string },
): Promise<boolean> {
  const mode = gConfig?.resultMode ?? "off";
  if (mode === "off" || !orchestrator) {
    return false;
  }
  const timestamp = Date.now();
  try {
    const decision = await orchestrator.checkResult({ tool, args, result, context });
    const redacted = mode === "enforce" && decision.decision === "deny";
    recordGuardrailResultDecision({
      tool,
      args,
      decision: decision.decision,
      reason: "reason" in decision ? decision.reason : undefined,
      mode,
      failClosed: gConfig?.failClosed ?? false,
      redacted,
      timestamp,
      sessionKey: context.sessionKey,
      agentId: context.agentId,
      category: "result",
    });

    // Sprint 9: Emit event to store for UI surfacing
    if (mode === "warn" || mode === "enforce") {
      const store = getGuardrailEventsStore();
      store.addEvent({
        type: "result",
        decision: decision.decision,
        reason: "reason" in decision ? decision.reason : undefined,
        summary: `Result from ${tool}`,
        tool,
        args,
        riskLevel: undefined, // Results don't have risk level in current model
        sessionKey: context.sessionKey,
        agentId: context.agentId,
      });
    }
    if (mode === "warn") {
      console.info(
        `Guardrail result decision: ${decision.decision}${"reason" in decision ? ` — ${decision.reason}` : ""}`,
      );
      return false;
    }
    return redacted;
  } catch (err) {
    const redacted = gConfig?.failClosed ?? false;
    recordGuardrailResultDecision({
      tool,
      args,
      decision: "deny",
      reason: (err as Error)?.message ?? String(err),
      mode,
      failClosed: gConfig?.failClosed ?? false,
      redacted,
      timestamp,
      sessionKey: context.sessionKey,
      agentId: context.agentId,
      category: "result",
    });
    if (gConfig?.failClosed) {
      return true;
    }
    console.warn(`Guardrail checkResult error (showing result): ${(err as Error)?.message ?? err}`);
    return false;
  }
}
