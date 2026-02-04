import type { ErrorObject } from "ajv";
import type { GatewayRequestContext, RespondFn } from "./types.js";
import type { GuardrailsConfig } from "../../config/guardrails.js";
import { resolveGuardrailsConfig } from "../../config/guardrails.js";
import { recordGuardrailNodeInvoke } from "../../guardrails/audit.js";
import type { PolicyDecision, ProposedAction } from "../../guardrails/orchestrator.js";
import crypto from "node:crypto";
import { ErrorCodes, errorShape, formatValidationErrors } from "../protocol/index.js";
import { formatForLog } from "../ws-log.js";

export type NodeInvokeGuardResult = {
  proceed: boolean;
  forceApproval?: boolean;
  decision?: PolicyDecision;
};

type ValidatorFn = ((value: unknown) => boolean) & {
  errors?: ErrorObject[] | null;
};

export function respondInvalidParams(params: {
  respond: RespondFn;
  method: string;
  validator: ValidatorFn;
}) {
  params.respond(
    false,
    undefined,
    errorShape(
      ErrorCodes.INVALID_REQUEST,
      `invalid ${params.method} params: ${formatValidationErrors(params.validator.errors)}`,
    ),
  );
}

export async function respondUnavailableOnThrow(respond: RespondFn, fn: () => Promise<void>) {
  try {
    await fn();
  } catch (err) {
    respond(false, undefined, errorShape(ErrorCodes.UNAVAILABLE, formatForLog(err)));
  }
}

export function uniqueSortedStrings(values: unknown[]) {
  return [...new Set(values.filter((v) => typeof v === "string"))]
    .map((v) => v.trim())
    .filter(Boolean)
    .toSorted();
}

export function safeParseJson(value: string | null | undefined): unknown {
  if (typeof value !== "string") {
    return undefined;
  }
  const trimmed = value.trim();
  if (!trimmed) {
    return undefined;
  }
  try {
    return JSON.parse(trimmed) as unknown;
  } catch {
    return { payloadJSON: value };
  }
}

/**
 * OXCER: Run guardrail action check for a node invoke. Returns a result object so callers can
 * use result.proceed to decide whether to call nodeRegistry.invoke; forceApproval/decision
 * are set for needs_human to support future node-level approval flows.
 */
export async function runNodeInvokeGuard(
  context: GatewayRequestContext,
  cfg: { guardrails?: GuardrailsConfig } | null | undefined,
  params: { nodeId: string; command: string; params?: unknown },
  respond: RespondFn,
): Promise<NodeInvokeGuardResult> {
  const gConfig = resolveGuardrailsConfig(cfg);
  const orchestrator = context.deps.guardrailOrchestrator;
  const timestamp = Date.now();
  if (gConfig.actionMode !== "enforce" || !orchestrator) {
    if (gConfig.actionMode === "warn" && orchestrator) {
      const action: ProposedAction = {
        id: crypto.randomUUID(),
        tool: "node.invoke",
        args: { nodeId: params.nodeId, command: params.command, params: params.params },
        summary: `node.invoke ${params.nodeId} ${params.command}`,
        timestamp,
      };
      void orchestrator
        .checkAction({ action })
        .then((d) => {
          recordGuardrailNodeInvoke({
            nodeId: params.nodeId,
            command: params.command,
            params: params.params,
            decision: d.decision,
            reason: "reason" in d ? d.reason : undefined,
            proceed: true,
            forceApproval: d.decision === "needs_human",
            timestamp,
            category: "node.invoke",
          });
          context.logGateway.info(
            `Guardrail node.invoke decision: ${d.decision}${"reason" in d ? ` — ${d.reason}` : ""}`,
          );
        })
        .catch((e) => {
          context.logGateway.warn(`Guardrail checkAction error: ${(e as Error)?.message ?? e}`);
        });
    }
    return { proceed: true };
  }
  const action: ProposedAction = {
    id: crypto.randomUUID(),
    tool: "node.invoke",
    args: { nodeId: params.nodeId, command: params.command, params: params.params },
    summary: `node.invoke ${params.nodeId} ${params.command}`,
    timestamp,
  };
  try {
    const decision = await orchestrator.checkAction({ action });
    if (decision.decision === "deny") {
      recordGuardrailNodeInvoke({
        nodeId: params.nodeId,
        command: params.command,
        params: params.params,
        decision: "deny",
        reason: decision.reason,
        proceed: false,
        timestamp,
        category: "node.invoke",
      });
      respond(
        false,
        undefined,
        errorShape(ErrorCodes.INVALID_REQUEST, "node.invoke denied by guardrails", {
          details: { reason: decision.reason },
        }),
      );
      return { proceed: false, decision };
    }
    if (decision.decision === "needs_human") {
      recordGuardrailNodeInvoke({
        nodeId: params.nodeId,
        command: params.command,
        params: params.params,
        decision: "needs_human",
        reason: decision.reason,
        proceed: false,
        forceApproval: true,
        timestamp,
        category: "node.invoke",
      });
      respond(
        false,
        undefined,
        errorShape(ErrorCodes.INVALID_REQUEST, "Human approval required", {
          details: { reason: decision.reason, summary: decision.summary },
        }),
      );
      return { proceed: false, forceApproval: true, decision };
    }
    recordGuardrailNodeInvoke({
      nodeId: params.nodeId,
      command: params.command,
      params: params.params,
      decision: "allow",
      proceed: true,
      timestamp,
      category: "node.invoke",
    });
    return { proceed: true, decision };
  } catch (err) {
    if (gConfig.failClosed) {
      recordGuardrailNodeInvoke({
        nodeId: params.nodeId,
        command: params.command,
        params: params.params,
        proceed: false,
        timestamp,
        category: "node.invoke",
        errorMessage: (err as Error)?.message ?? String(err),
      });
      respond(
        false,
        undefined,
        errorShape(ErrorCodes.INVALID_REQUEST, "Guardrail check failed (fail-closed)", {
          details: { error: (err as Error)?.message ?? String(err) },
        }),
      );
      return { proceed: false };
    }
    context.logGateway.debug(
      `Guardrail checkAction error (continuing): ${(err as Error)?.message ?? err}`,
    );
    return { proceed: true };
  }
}
