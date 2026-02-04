/**
 * Guardrail orchestrator interface and no-op implementation.
 * OXCER: Central hook for pre-LLM, pre-exec, and post-exec policy checks.
 */

import { NemoGuardrailOrchestrator } from "./nemo-orchestrator.js";

export type PolicyDecision =
  | { decision: "allow"; reason?: string }
  | { decision: "deny"; reason: string }
  | { decision: "needs_human"; reason: string; summary: string };

export interface ProposedAction {
  id: string;
  tool: string;
  args: Record<string, unknown>;
  summary?: string;
  riskLevel?: "low" | "medium" | "high" | "critical";
  sessionKey?: string;
  agentId?: string;
  channel?: string;
  timestamp: number;
}

export interface GuardrailOrchestrator {
  checkPrompt(input: {
    messages: unknown[];
    context?: Record<string, unknown>;
  }): Promise<PolicyDecision>;

  checkAction(input: { action: ProposedAction }): Promise<PolicyDecision>;

  checkResult(input: {
    tool: string;
    args: Record<string, unknown>;
    result: unknown;
    context?: Record<string, unknown>;
  }): Promise<PolicyDecision>;
}

export class NoopGuardrailOrchestrator implements GuardrailOrchestrator {
  async checkPrompt(): Promise<PolicyDecision> {
    return { decision: "allow" };
  }

  async checkAction(): Promise<PolicyDecision> {
    return { decision: "allow" };
  }

  async checkResult(): Promise<PolicyDecision> {
    return { decision: "allow" };
  }
}

/**
 * Factory function to create a guardrail orchestrator based on configuration.
 * OXCER: Selects between noop and NeMo engines based on guardrails.textguardrails.engine.
 */
export function createGuardrailOrchestrator(
  config:
    | {
        guardrails?: {
          textguardrails?: {
            engine?: "noop" | "nemo";
            nemo?: { endpoint?: string; apiKey?: string };
          };
        };
      }
    | null
    | undefined,
): GuardrailOrchestrator {
  const engine = config?.guardrails?.textguardrails?.engine ?? "noop";

  if (engine === "nemo") {
    const nemoConfig = config?.guardrails?.textguardrails?.nemo;
    const endpoint = nemoConfig?.endpoint ?? "http://localhost:8080";
    return new NemoGuardrailOrchestrator({
      endpoint,
      apiKey: nemoConfig?.apiKey,
    });
  }

  return new NoopGuardrailOrchestrator();
}
