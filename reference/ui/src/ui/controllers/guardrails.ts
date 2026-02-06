/**
 * OXCER: Guardrails events controller for UI.
 * Sprint 9: Simple controller for listing events and submitting reviews.
 */

import type { Gateway } from "../gateway.js";

export type GuardrailEvent = {
  id: string;
  timestamp: number;
  type: "action" | "result";
  decision: "allow" | "deny" | "needs_human";
  reason?: string;
  summary: string;
  tool: string;
  args: Record<string, unknown>;
  riskLevel?: "low" | "medium" | "high" | "critical";
  status: "pending_review" | "resolved" | "auto_resolved";
  humanDecision?: "approve" | "reject";
  humanReviewer?: string;
  humanReviewNote?: string;
  humanReviewedAt?: number;
};

export async function listGuardrailEvents(
  gateway: Gateway,
  opts?: { status?: "pending_review" | "resolved" | "auto_resolved"; limit?: number; sessionKey?: string },
): Promise<GuardrailEvent[]> {
  const result = await gateway.request("guardrails.events.list", opts ?? {});
  if (!result.ok || !result.payload) {
    return [];
  }
  const payload = result.payload as { events?: GuardrailEvent[] };
  return payload.events ?? [];
}

export async function getGuardrailEvent(gateway: Gateway, id: string): Promise<GuardrailEvent | null> {
  const result = await gateway.request("guardrails.events.get", { id });
  if (!result.ok || !result.payload) {
    return null;
  }
  const payload = result.payload as { event?: GuardrailEvent };
  return payload.event ?? null;
}

export async function submitGuardrailReview(
  gateway: Gateway,
  id: string,
  decision: "approve" | "reject",
  note?: string,
  reviewer?: string,
): Promise<boolean> {
  const result = await gateway.request("guardrails.events.review", {
    id,
    decision,
    note,
    reviewer,
  });
  return result.ok;
}

export async function getPendingReviews(gateway: Gateway): Promise<GuardrailEvent[]> {
  const result = await gateway.request("guardrails.reviews.pending", {});
  if (!result.ok || !result.payload) {
    return [];
  }
  const payload = result.payload as { events?: GuardrailEvent[] };
  return payload.events ?? [];
}
