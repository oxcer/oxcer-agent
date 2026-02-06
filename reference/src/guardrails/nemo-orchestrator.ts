/**
 * OXCER: NeMo Guardrails orchestrator implementation.
 * Calls out to a NeMo Guardrails service (REST or gRPC HTTP) and maps responses to PolicyDecision.
 *
 * Sprint 7: Basic integration with NeMo service.
 * Sprint 8: Enhanced logging and observability for guardrail decisions.
 */

import type { GuardrailOrchestrator, PolicyDecision, ProposedAction } from "./orchestrator.js";

export type NemoGuardrailsConfig = {
  /** NeMo Guardrails service endpoint (e.g., "http://localhost:8080"). */
  endpoint: string;
  /** Optional API key for authentication (for future use). */
  apiKey?: string;
};

/**
 * NeMo Guardrails orchestrator that calls an external NeMo Guardrails service.
 *
 * Expected NeMo API:
 * POST {endpoint}/guardrails/check_action
 * Request: { action: string, args: any, context: any }
 * Response: { decision: "allow" | "deny" | "needs_human", reason?: string }
 *
 * This implementation is isolated so the HTTP client logic can be swapped later
 * to match the real NeMo Guardrails API.
 */
export class NemoGuardrailOrchestrator implements GuardrailOrchestrator {
  private readonly config: NemoGuardrailsConfig;
  private readonly baseUrl: string;
  private readonly isDev: boolean;

  constructor(config: NemoGuardrailsConfig) {
    this.config = config;
    this.baseUrl = config.endpoint.replace(/\/$/, "");
    this.isDev = process.env.NODE_ENV === "development";
  }

  /**
   * Redact sensitive information from args for logging.
   */
  private redactArgs(args: Record<string, unknown>): Record<string, unknown> {
    const redacted: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(args)) {
      if (typeof value === "string" && value.length > 200) {
        redacted[key] = `${value.substring(0, 100)}... [truncated ${value.length} chars]`;
      } else {
        redacted[key] = value;
      }
    }
    return redacted;
  }

  async checkPrompt(input: {
    messages: unknown[];
    context?: Record<string, unknown>;
  }): Promise<PolicyDecision> {
    // For now, prompt checks are not implemented for NeMo.
    // This can be extended later when NeMo prompt guardrails are needed.
    return { decision: "allow" };
  }

  async checkAction(input: { action: ProposedAction }): Promise<PolicyDecision> {
    const { action } = input;
    const url = `${this.baseUrl}/guardrails/check_action`;
    const startTime = Date.now();

    // Log request summary (structured logging for observability)
    const redactedArgs = this.redactArgs(action.args);
    if (this.isDev) {
      console.debug(`[NeMo Guardrails] checkAction: ${action.tool}`, {
        tool: action.tool,
        args: redactedArgs,
        summary: action.summary,
        riskLevel: action.riskLevel,
        agentId: action.agentId,
        sessionKey: action.sessionKey ? `${action.sessionKey.substring(0, 8)}...` : undefined,
      });
    }

    try {
      const requestBody = {
        action: action.tool,
        args: action.args,
        context: {
          summary: action.summary,
          riskLevel: action.riskLevel,
          sessionKey: action.sessionKey,
          agentId: action.agentId,
          channel: action.channel,
          timestamp: action.timestamp,
        },
      };

      const headers: Record<string, string> = {
        "Content-Type": "application/json",
      };
      if (this.config.apiKey) {
        headers["Authorization"] = `Bearer ${this.config.apiKey}`;
      }

      const response = await fetch(url, {
        method: "POST",
        headers,
        body: JSON.stringify(requestBody),
      });

      if (!response.ok) {
        const errorText = await response.text().catch(() => "Unknown error");
        const errorMessage = `NeMo Guardrails service error (${response.status}): ${errorText}`;
        if (this.isDev) {
          console.error(`[NeMo Guardrails] checkAction failed:`, {
            tool: action.tool,
            status: response.status,
            error: errorText,
          });
        }
        throw new Error(errorMessage);
      }

      const result = await response.json();
      const duration = Date.now() - startTime;

      // Map NeMo response to PolicyDecision
      // Expected: { decision: "allow" | "deny" | "needs_human", reason?: string, log?: object }
      const decision = result.decision as string;
      const reason = result.reason as string | undefined;
      const nemoLog = result.log as Record<string, unknown> | undefined;

      // Log decision with structured data for observability
      if (this.isDev || decision !== "allow") {
        console.info(`[NeMo Guardrails] checkAction decision: ${decision.toUpperCase()}`, {
          tool: action.tool,
          decision,
          reason,
          duration: `${duration}ms`,
          riskLevel: action.riskLevel,
          // Include NeMo's own log fields if available (for correlation with NeMo logs)
          nemoLog: nemoLog ? { traceId: nemoLog.traceId, step: nemoLog.step } : undefined,
        });
      }

      if (decision === "allow") {
        return { decision: "allow", reason };
      }
      if (decision === "deny" || decision === "blocked") {
        return { decision: "deny", reason: reason ?? "Action denied by NeMo Guardrails" };
      }
      if (decision === "needs_human" || decision === "uncertain" || decision === "needs_review") {
        return {
          decision: "needs_human",
          reason: reason ?? "Action requires human review",
          summary: action.summary ?? `${action.tool} action`,
        };
      }

      // Unknown decision type - log and default to allow (fail-open)
      console.warn(`[NeMo Guardrails] Unknown decision type: ${decision}`, {
        tool: action.tool,
        rawResult: result,
      });
      return { decision: "allow", reason: `Unknown NeMo decision: ${decision}` };
    } catch (err) {
      const errorMessage = (err as Error)?.message ?? String(err);
      const duration = Date.now() - startTime;
      // Log error with structured data
      console.error(`[NeMo Guardrails] checkAction error:`, {
        tool: action.tool,
        error: errorMessage,
        duration: `${duration}ms`,
      });
      // Re-throw so callers can handle fail-closed behavior
      throw new Error(`NeMo Guardrails checkAction failed: ${errorMessage}`);
    }
  }

  async checkResult(input: {
    tool: string;
    args: Record<string, unknown>;
    result: unknown;
    context?: Record<string, unknown>;
  }): Promise<PolicyDecision> {
    const { tool, args, result, context } = input;
    const url = `${this.baseUrl}/guardrails/check_result`;
    const startTime = Date.now();

    // Log request summary (structured logging for observability)
    const redactedArgs = this.redactArgs(args);
    const resultPreview =
      typeof result === "string" && result.length > 200
        ? `${result.substring(0, 100)}... [truncated ${result.length} chars]`
        : result;
    if (this.isDev) {
      console.debug(`[NeMo Guardrails] checkResult: ${tool}`, {
        tool,
        args: redactedArgs,
        resultPreview,
        sessionKey: context?.sessionKey
          ? `${String(context.sessionKey).substring(0, 8)}...`
          : undefined,
        agentId: context?.agentId,
      });
    }

    try {
      const requestBody = {
        tool,
        args,
        result,
        context: context ?? {},
      };

      const headers: Record<string, string> = {
        "Content-Type": "application/json",
      };
      if (this.config.apiKey) {
        headers["Authorization"] = `Bearer ${this.config.apiKey}`;
      }

      const response = await fetch(url, {
        method: "POST",
        headers,
        body: JSON.stringify(requestBody),
      });

      if (!response.ok) {
        const errorText = await response.text().catch(() => "Unknown error");
        const errorMessage = `NeMo Guardrails service error (${response.status}): ${errorText}`;
        if (this.isDev) {
          console.error(`[NeMo Guardrails] checkResult failed:`, {
            tool,
            status: response.status,
            error: errorText,
          });
        }
        throw new Error(errorMessage);
      }

      const resultData = await response.json();
      const duration = Date.now() - startTime;

      // Map NeMo response to PolicyDecision
      const decision = resultData.decision as string;
      const reason = resultData.reason as string | undefined;
      const nemoLog = resultData.log as Record<string, unknown> | undefined;

      // Log decision with structured data for observability
      if (this.isDev || decision !== "allow") {
        console.info(`[NeMo Guardrails] checkResult decision: ${decision.toUpperCase()}`, {
          tool,
          decision,
          reason,
          duration: `${duration}ms`,
          // Include NeMo's own log fields if available (for correlation with NeMo logs)
          nemoLog: nemoLog ? { traceId: nemoLog.traceId, step: nemoLog.step } : undefined,
        });
      }

      if (decision === "allow") {
        return { decision: "allow", reason };
      }
      if (decision === "deny" || decision === "blocked") {
        return { decision: "deny", reason: reason ?? "Result denied by NeMo Guardrails" };
      }
      if (decision === "needs_human" || decision === "uncertain" || decision === "needs_review") {
        return {
          decision: "needs_human",
          reason: reason ?? "Result requires human review",
          summary: `Result from ${tool}`,
        };
      }

      // Unknown decision type - log and default to allow (fail-open)
      console.warn(`[NeMo Guardrails] Unknown decision type: ${decision}`, {
        tool,
        rawResult: resultData,
      });
      return { decision: "allow", reason: `Unknown NeMo decision: ${decision}` };
    } catch (err) {
      const errorMessage = (err as Error)?.message ?? String(err);
      const duration = Date.now() - startTime;
      // Log error with structured data
      console.error(`[NeMo Guardrails] checkResult error:`, {
        tool,
        error: errorMessage,
        duration: `${duration}ms`,
      });
      // Re-throw so callers can handle fail-closed behavior
      throw new Error(`NeMo Guardrails checkResult failed: ${errorMessage}`);
    }
  }
}
