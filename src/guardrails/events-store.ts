/**
 * OXCER: In-memory store for guardrail events and human review queue.
 * Sprint 9: Simple store for UI surfacing; not durable across restarts.
 */

import type { PolicyDecision } from "./orchestrator.js";

export type GuardrailEventType = "action" | "result";

export type GuardrailEventStatus = "pending_review" | "resolved" | "auto_resolved";

export type GuardrailEvent = {
  id: string;
  timestamp: number;
  type: GuardrailEventType;
  decision: PolicyDecision["decision"];
  reason?: string;
  summary: string;
  tool: string;
  args: Record<string, unknown>;
  riskLevel?: "low" | "medium" | "high" | "critical";
  status: GuardrailEventStatus;
  sessionKey?: string;
  agentId?: string;
  channel?: string;
  // Human review fields (set when reviewed)
  humanDecision?: "approve" | "reject";
  humanReviewer?: string;
  humanReviewNote?: string;
  humanReviewedAt?: number;
};

class GuardrailEventsStore {
  private events: Map<string, GuardrailEvent> = new Map();
  private maxEvents = 1000; // Keep last 1000 events

  /**
   * Add a new guardrail event. If decision is "needs_human", status is set to "pending_review".
   */
  addEvent(event: Omit<GuardrailEvent, "id" | "timestamp" | "status">): string {
    const id = this.generateId();
    const timestamp = Date.now();
    const status: GuardrailEventStatus =
      event.decision === "needs_human" ? "pending_review" : "resolved";

    const fullEvent: GuardrailEvent = {
      ...event,
      id,
      timestamp,
      status,
    };

    this.events.set(id, fullEvent);

    // Trim old events if we exceed maxEvents
    if (this.events.size > this.maxEvents) {
      const sorted = Array.from(this.events.values()).sort((a, b) => a.timestamp - b.timestamp);
      const toRemove = sorted.slice(0, this.events.size - this.maxEvents);
      for (const event of toRemove) {
        this.events.delete(event.id);
      }
    }

    return id;
  }

  /**
   * Get all events, optionally filtered by status and sessionKey.
   */
  getEvents(opts?: {
    status?: GuardrailEventStatus;
    limit?: number;
    sessionKey?: string;
  }): GuardrailEvent[] {
    let events = Array.from(this.events.values());

    if (opts?.status) {
      events = events.filter((e) => e.status === opts.status);
    }

    const sessionKey = opts?.sessionKey?.trim();
    if (sessionKey) {
      events = events.filter((e) => e.sessionKey === sessionKey);
    }

    // Sort by timestamp descending (newest first)
    events.sort((a, b) => b.timestamp - a.timestamp);

    if (opts?.limit) {
      events = events.slice(0, opts.limit);
    }

    return events;
  }

  /**
   * Get a single event by ID.
   */
  getEvent(id: string): GuardrailEvent | undefined {
    return this.events.get(id);
  }

  /**
   * Submit a human review for a needs_human event.
   */
  submitReview(
    eventId: string,
    decision: "approve" | "reject",
    reviewer: string,
    note?: string,
  ): boolean {
    const event = this.events.get(eventId);
    if (!event) {
      return false;
    }
    if (event.status !== "pending_review") {
      return false; // Already resolved
    }

    event.humanDecision = decision;
    event.humanReviewer = reviewer;
    event.humanReviewNote = note;
    event.humanReviewedAt = Date.now();
    event.status = "resolved";

    return true;
  }

  /**
   * Mark an event as auto-resolved (e.g., timeout or automatic resolution).
   */
  markAutoResolved(eventId: string): boolean {
    const event = this.events.get(eventId);
    if (!event) {
      return false;
    }
    if (event.status !== "pending_review") {
      return false;
    }

    event.status = "auto_resolved";
    return true;
  }

  /**
   * Get pending review queue (needs_human events that haven't been resolved).
   */
  getPendingReviews(): GuardrailEvent[] {
    return this.getEvents({ status: "pending_review" });
  }

  private generateId(): string {
    return `guardrail-${Date.now()}-${Math.random().toString(36).substring(2, 11)}`;
  }
}

// Singleton instance
let store: GuardrailEventsStore | null = null;

export function getGuardrailEventsStore(): GuardrailEventsStore {
  if (!store) {
    store = new GuardrailEventsStore();
  }
  return store;
}

/**
 * Reset the store (useful for tests or explicit cleanup).
 */
export function resetGuardrailEventsStore(): void {
  store = null;
}
