/**
 * OXCER: Guardrails UI view.
 * Sprint 9: Simple panel showing guardrail decisions and review queue.
 */

import { html, nothing } from "lit";
import type { AppViewState } from "../app-view-state.js";
import type { GuardrailEvent } from "../controllers/guardrails.js";

function formatTimestamp(ms: number): string {
  const date = new Date(ms);
  return date.toLocaleString();
}

function formatDecision(decision: string): string {
  return decision.toUpperCase();
}

function formatRiskLevel(risk?: string): string {
  if (!risk) return "";
  return risk.charAt(0).toUpperCase() + risk.slice(1);
}

function redactArgs(args: Record<string, unknown>): string {
  const redacted: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(args)) {
    if (typeof value === "string" && value.length > 50) {
      redacted[key] = `${value.substring(0, 30)}...`;
    } else {
      redacted[key] = value;
    }
  }
  return JSON.stringify(redacted, null, 2);
}

function renderEventRow(event: GuardrailEvent, state: AppViewState) {
  const isPending = event.status === "pending_review";
  const decisionClass = event.decision === "deny" ? "danger" : event.decision === "needs_human" ? "warning" : "";
  return html`
    <tr class=${decisionClass} ?data-pending=${isPending}>
      <td>${formatTimestamp(event.timestamp)}</td>
      <td>${event.type}</td>
      <td><strong>${formatDecision(event.decision)}</strong></td>
      <td>${event.tool}</td>
      <td>${event.summary || "-"}</td>
      <td>${formatRiskLevel(event.riskLevel)}</td>
      <td>
        ${isPending
          ? html`<button class="btn small" @click=${() => state.selectGuardrailEvent(event.id)}>
              Review
            </button>`
          : event.humanDecision
            ? html`<span class="badge">${event.humanDecision === "approve" ? "✓ Approved" : "✗ Rejected"}</span>`
            : nothing}
      </td>
    </tr>
  `;
}

export function renderGuardrailsView(state: AppViewState) {
  const events = state.guardrailEvents;
  const pendingCount = events.filter((e) => e.status === "pending_review").length;
  const selectedEvent = state.selectedGuardrailEvent
    ? events.find((e) => e.id === state.selectedGuardrailEvent)
    : null;

  return html`
    <div class="guardrails-view">
      <div class="guardrails-header">
        <h2>Guardrails</h2>
        ${pendingCount > 0
          ? html`<div class="guardrails-pending-badge">${pendingCount} pending review</div>`
          : nothing}
        <button class="btn" @click=${() => state.refreshGuardrailEvents()}>Refresh</button>
      </div>

      ${selectedEvent && selectedEvent.status === "pending_review"
        ? html`
            <div class="guardrails-review-card">
              <h3>Review: ${selectedEvent.summary}</h3>
              <div class="guardrails-review-details">
                <div><strong>Tool:</strong> ${selectedEvent.tool}</div>
                <div><strong>Type:</strong> ${selectedEvent.type}</div>
                <div><strong>Risk Level:</strong> ${formatRiskLevel(selectedEvent.riskLevel) || "Unknown"}</div>
                ${selectedEvent.reason ? html`<div><strong>Reason:</strong> ${selectedEvent.reason}</div>` : nothing}
                <div><strong>Arguments:</strong></div>
                <pre class="guardrails-args">${redactArgs(selectedEvent.args)}</pre>
              </div>
              <div class="guardrails-review-actions">
                <button
                  class="btn primary"
                  ?disabled=${state.guardrailReviewBusy}
                  @click=${() => state.submitGuardrailReview(selectedEvent.id, "approve")}
                >
                  Approve
                </button>
                <button
                  class="btn danger"
                  ?disabled=${state.guardrailReviewBusy}
                  @click=${() => state.submitGuardrailReview(selectedEvent.id, "reject")}
                >
                  Reject
                </button>
                <button class="btn" @click=${() => state.selectGuardrailEvent(null)}>Cancel</button>
              </div>
            </div>
          `
        : nothing}

      <div class="guardrails-table-container">
        <table class="guardrails-table">
          <thead>
            <tr>
              <th>Time</th>
              <th>Type</th>
              <th>Decision</th>
              <th>Tool</th>
              <th>Summary</th>
              <th>Risk</th>
              <th>Action</th>
            </tr>
          </thead>
          <tbody>
            ${events.length === 0
              ? html`<tr><td colspan="7" class="empty">No guardrail events yet</td></tr>`
              : events.map((event) => renderEventRow(event, state))}
          </tbody>
        </table>
      </div>
    </div>
  `;
}
