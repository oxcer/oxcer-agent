/**
 * OXCER: Minimal monitoring hook on top of the audit trail. No-op in this sprint;
 * in future sprints can send to StatsD, Prometheus, or other backends.
 */

/** Event payload written to JSONL; passed through for metrics. */
export type GuardrailMetricEvent = Record<string, unknown> & {
  eventType?: string;
  tool?: string;
  command?: string;
  decision?: string;
  riskLevel?: string;
};

/** Severity bucket for alerting (derived from riskLevel / decision). */
export type GuardrailSeverity = "low" | "medium" | "high" | "critical";

function severityFromEvent(event: GuardrailMetricEvent): GuardrailSeverity {
  const risk = event.riskLevel as string | undefined;
  if (risk === "critical") return "critical";
  if (risk === "high") return "high";
  if (risk === "medium") return "medium";
  if (event.decision === "deny" || event.decision === "needs_human") return "high";
  return "low";
}

/**
 * Called from each recordGuardrail* after successfully writing to JSONL.
 * For now: no-op or console.debug. Future: send to metrics backend.
 * Derived fields (e.g. severity, deny/needs_human counts per tool) can be
 * computed here for alerting.
 */
export function emitGuardrailMetric(event: GuardrailMetricEvent): void {
  if (process.env.NODE_ENV === "development") {
    const severity = severityFromEvent(event);
    console.debug(
      "[guardrail:metric]",
      event.eventType,
      event.tool ?? event.command,
      event.decision,
      severity,
    );
  }
}
