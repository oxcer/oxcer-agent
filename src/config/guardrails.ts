/**
 * OXCER: Guardrails configuration and resolver.
 * Modes: off (skip), warn (log only), enforce (apply allow/deny/needs_human).
 */

import path from "node:path";

export type GuardrailMode = "off" | "warn" | "enforce";

export type GuardrailsAuditConfig = {
  /** If false, no JSONL audit file is written. Default: true in prod, configurable in dev. */
  enabled?: boolean;
  /** Override path for append-only JSONL file. Default: $OPENCLAW_STATE_DIR/audit/actions.jsonl */
  filePath?: string;
  /** When true, also log each event to console.debug (useful in dev). */
  devConsole?: boolean;
};

export type ResolvedGuardrailsAuditConfig = {
  enabled: boolean;
  filePath: string;
  devConsole: boolean;
};

export type TextGuardrailsConfig = {
  /** Guardrails engine to use: "noop" (default, no external calls) or "nemo" (NeMo Guardrails service). */
  engine?: "noop" | "nemo";
  /** NeMo Guardrails service configuration (required when engine is "nemo"). */
  nemo?: {
    /** NeMo Guardrails service endpoint (e.g., "http://localhost:8080"). */
    endpoint?: string;
    /** Optional API key for authentication (for future use). */
    apiKey?: string;
  };
};

export type GuardrailsConfig = {
  action?: { mode?: GuardrailMode };
  result?: { mode?: GuardrailMode };
  /** When true, treat guardrail check errors as deny (fail-closed). */
  failClosed?: boolean;
  /** Audit trail and monitoring. Supports enterprise compliance and incident forensics. */
  audit?: GuardrailsAuditConfig;
  /** Text guardrails engine configuration (for external policy engines like NeMo). */
  textguardrails?: TextGuardrailsConfig;
};

export type ResolvedGuardrailsConfig = {
  actionMode: GuardrailMode;
  resultMode: GuardrailMode;
  failClosed: boolean;
};

const DEFAULT_ACTION_MODE: GuardrailMode = "off";
const DEFAULT_RESULT_MODE: GuardrailMode = "off";
const DEFAULT_FAIL_CLOSED = false;

export function resolveGuardrailsConfig(
  cfg: { guardrails?: GuardrailsConfig } | null | undefined,
): ResolvedGuardrailsConfig {
  const g = cfg?.guardrails;
  return {
    actionMode: (g?.action?.mode as GuardrailMode | undefined) ?? DEFAULT_ACTION_MODE,
    resultMode: (g?.result?.mode as GuardrailMode | undefined) ?? DEFAULT_RESULT_MODE,
    failClosed: g?.failClosed ?? DEFAULT_FAIL_CLOSED,
  };
}

const DEFAULT_AUDIT_ENABLED = true;
const DEFAULT_AUDIT_DEV_CONSOLE = false;

export function resolveGuardrailsAuditConfig(
  cfg: { guardrails?: GuardrailsConfig } | null | undefined,
  stateDir: string,
): ResolvedGuardrailsAuditConfig {
  const g = cfg?.guardrails;
  const audit = g?.audit;
  const defaultPath = path.join(stateDir, "audit", "actions.jsonl");
  return {
    enabled: audit?.enabled ?? DEFAULT_AUDIT_ENABLED,
    filePath: audit?.filePath?.trim() ? audit.filePath : defaultPath,
    devConsole: audit?.devConsole ?? DEFAULT_AUDIT_DEV_CONSOLE,
  };
}
