/**
 * OXCER: Gateway methods for guardrails events and human review queue.
 * Sprint 9: Exposes guardrail decisions and allows human review via WebSocket API.
 */

import type { GatewayRequestHandlers, RespondFn } from "./types.js";
import { getGuardrailEventsStore } from "../../guardrails/events-store.js";
import { recordGuardrailHumanReview } from "../../guardrails/audit.js";
import { ErrorCodes, errorShape } from "../protocol/index.js";

export const guardrailsHandlers: GatewayRequestHandlers = {
  /**
   * List recent guardrail events.
   * Params: { status?: "pending_review" | "resolved" | "auto_resolved", limit?: number, sessionKey?: string }
   */
  "guardrails.events.list": ({ params, respond }) => {
    const store = getGuardrailEventsStore();
    const status = (params as { status?: string })?.status as
      | "pending_review"
      | "resolved"
      | "auto_resolved"
      | undefined;
    const limit =
      typeof (params as { limit?: unknown })?.limit === "number"
        ? (params as { limit: number }).limit
        : undefined;
    const sessionKey =
      typeof (params as { sessionKey?: unknown })?.sessionKey === "string"
        ? (params as { sessionKey: string }).sessionKey.trim() || undefined
        : undefined;

    const events = store.getEvents({ status, limit: limit ?? 100, sessionKey });
    respond(true, { events });
  },

  /**
   * Get a single guardrail event by ID.
   * Params: { id: string }
   */
  "guardrails.events.get": ({ params, respond }) => {
    const id = (params as { id?: unknown })?.id;
    if (typeof id !== "string" || !id.trim()) {
      respond(
        false,
        undefined,
        errorShape(ErrorCodes.INVALID_REQUEST, "guardrails.events.get requires id parameter"),
      );
      return;
    }

    const store = getGuardrailEventsStore();
    const event = store.getEvent(id.trim());
    if (!event) {
      respond(
        false,
        undefined,
        errorShape(ErrorCodes.INVALID_REQUEST, `Guardrail event not found: ${id}`),
      );
      return;
    }

    respond(true, { event });
  },

  /**
   * Submit a human review for a needs_human event.
   * Params: { id: string, decision: "approve" | "reject", note?: string, reviewer?: string }
   */
  "guardrails.events.review": ({ params, respond, context }) => {
    const id = (params as { id?: unknown })?.id;
    const decision = (params as { decision?: unknown })?.decision;
    const note = (params as { note?: unknown })?.note;
    const reviewer = (params as { reviewer?: unknown })?.reviewer;

    if (typeof id !== "string" || !id.trim()) {
      respond(
        false,
        undefined,
        errorShape(ErrorCodes.INVALID_REQUEST, "guardrails.events.review requires id parameter"),
      );
      return;
    }

    if (decision !== "approve" && decision !== "reject") {
      respond(
        false,
        undefined,
        errorShape(
          ErrorCodes.INVALID_REQUEST,
          'guardrails.events.review requires decision: "approve" or "reject"',
        ),
      );
      return;
    }

    const store = getGuardrailEventsStore();
    const event = store.getEvent(id.trim());
    if (!event) {
      respond(
        false,
        undefined,
        errorShape(ErrorCodes.INVALID_REQUEST, `Guardrail event not found: ${id}`),
      );
      return;
    }

    if (event.status !== "pending_review") {
      respond(
        false,
        undefined,
        errorShape(
          ErrorCodes.INVALID_REQUEST,
          `Event ${id} is not pending review (status: ${event.status})`,
        ),
      );
      return;
    }

    const reviewerId = typeof reviewer === "string" ? reviewer : "local-operator";
    const reviewNote = typeof note === "string" ? note : undefined;

    const success = store.submitReview(id.trim(), decision, reviewerId, reviewNote);
    if (!success) {
      respond(false, undefined, errorShape(ErrorCodes.UNAVAILABLE, "Failed to submit review"));
      return;
    }

    // Log human review to audit trail
    recordGuardrailHumanReview({
      eventId: event.id,
      originalDecision: event.decision,
      humanDecision: decision,
      reviewer: reviewerId,
      note: reviewNote,
      timestamp: Date.now(),
      tool: event.tool,
      summary: event.summary,
      riskLevel: event.riskLevel,
    });

    // Return updated event
    const updatedEvent = store.getEvent(id.trim());
    respond(true, { event: updatedEvent });
  },

  /**
   * Get pending review queue (needs_human events).
   * Params: none
   */
  "guardrails.reviews.pending": ({ respond }) => {
    const store = getGuardrailEventsStore();
    const pending = store.getPendingReviews();
    respond(true, { events: pending });
  },
};
