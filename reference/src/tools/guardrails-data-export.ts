#!/usr/bin/env node
/**
 * OXCER: Export normalized guardrails dataset from JSONL audit logs.
 * Sprint 10: Data pipeline foundation for tuning prep (no retraining in-sprint).
 *
 * Default input: guardrails.audit.filePath (resolved via config + STATE_DIR fallback).
 *
 * Usage:
 *   pnpm guardrails:data:export
 *   pnpm guardrails:data:export --input /path/to/actions.jsonl --output data/guardrails/normalized/guardrails-20260203-153000.jsonl
 */

import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { loadConfig, STATE_DIR } from "../config/config.js";
import { resolveGuardrailsAuditConfig } from "../config/guardrails.js";
import { tagFailureCategory } from "./guardrails-failure-tagger.js";

type GuardrailRecord = {
  id: string;
  timestamp: string;
  railType: "input" | "action" | "result";
  inputSummary: string;
  modelOutputSummary?: string;
  guardrailDecision: "allow" | "deny" | "needs_human";
  humanDecision?: "approve" | "reject";
  humanLabel?: "tp" | "tn" | "fp" | "fn";
  failureCategory?: string;
  meta?: Record<string, unknown>;
};

type AuditEvent = Record<string, unknown> & {
  eventId?: unknown;
  eventType?: unknown;
  timestamp?: unknown;
};

function parseArgs(argv: string[]) {
  const out: { input?: string; output?: string } = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--input") {
      out.input = argv[i + 1];
      i++;
      continue;
    }
    if (a === "--output") {
      out.output = argv[i + 1];
      i++;
      continue;
    }
  }
  return out;
}

function safeJsonParse(line: string): unknown {
  try {
    return JSON.parse(line);
  } catch {
    return null;
  }
}

function ensureDir(p: string): void {
  fs.mkdirSync(p, { recursive: true, mode: 0o755 });
}

function nowStamp(): string {
  // YYYYMMDD-HHMMSS
  const d = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}`;
}

function computeHumanLabel(params: {
  guardrailDecision: GuardrailRecord["guardrailDecision"];
  humanDecision: GuardrailRecord["humanDecision"];
}): GuardrailRecord["humanLabel"] {
  const g = params.guardrailDecision;
  const h = params.humanDecision;
  if (h === "approve") {
    // human says allow
    if (g === "deny") return "fp";
    if (g === "allow") return "tn";
    return "tp"; // needs_human that human approved -> treated as correct escalation
  }
  // human says reject (deny)
  if (g === "allow") return "fn";
  if (g === "deny") return "tp";
  return "tp"; // needs_human that human rejected -> correct escalation
}

function coerceDecision(raw: unknown): GuardrailRecord["guardrailDecision"] | null {
  if (raw === "allow" || raw === "deny" || raw === "needs_human") {
    return raw;
  }
  return null;
}

function redactSummary(summary: string): string {
  const s = summary.trim();
  if (s.length <= 240) return s;
  return `${s.slice(0, 220)}… [truncated]`;
}

function normalizeGuardrailEvent(ev: AuditEvent): GuardrailRecord | null {
  const eventType = typeof ev.eventType === "string" ? ev.eventType : "";
  const timestamp = typeof ev.timestamp === "string" ? ev.timestamp : "";
  const eventId = typeof ev.eventId === "string" ? ev.eventId : "";

  if (!eventId || !timestamp) return null;

  if (eventType === "guardrail.action") {
    const decision = coerceDecision(ev.decision);
    if (!decision) return null;
    const tool = typeof ev.tool === "string" ? ev.tool : "";
    const summary =
      typeof ev.summary === "string" ? ev.summary : tool ? `${tool} action` : "action";
    const args = (
      typeof ev.args === "object" && ev.args !== null ? (ev.args as Record<string, unknown>) : {}
    ) as Record<string, unknown>;
    const reason = typeof ev.reason === "string" ? ev.reason : undefined;

    const rec: GuardrailRecord = {
      id: eventId,
      timestamp,
      railType: "action",
      inputSummary: redactSummary(summary),
      guardrailDecision: decision,
      failureCategory:
        tagFailureCategory({ railType: "action", inputSummary: summary, tool, args, reason }) ??
        undefined,
      meta: {
        sourceEventType: eventType,
        tool,
        args,
        reason,
        riskLevel: ev.riskLevel,
        sessionKey: ev.sessionKey,
        agentId: ev.agentId,
        channel: ev.channel,
        mode: ev.mode,
        failClosed: ev.failClosed,
      },
    };
    return rec;
  }

  if (eventType === "guardrail.result") {
    const decision = coerceDecision(ev.decision);
    if (!decision) return null;
    const tool = typeof ev.tool === "string" ? ev.tool : "";
    const reason = typeof ev.reason === "string" ? ev.reason : undefined;
    const args = (
      typeof ev.args === "object" && ev.args !== null ? (ev.args as Record<string, unknown>) : {}
    ) as Record<string, unknown>;
    const summary = `Result from ${tool || "tool"}`;

    const rec: GuardrailRecord = {
      id: eventId,
      timestamp,
      railType: "result",
      inputSummary: redactSummary(summary),
      guardrailDecision: decision,
      failureCategory:
        tagFailureCategory({ railType: "result", inputSummary: summary, tool, args, reason }) ??
        undefined,
      meta: {
        sourceEventType: eventType,
        tool,
        args,
        reason,
        sessionKey: ev.sessionKey,
        agentId: ev.agentId,
        mode: ev.mode,
        failClosed: ev.failClosed,
        redacted: ev.redacted,
      },
    };
    return rec;
  }

  return null;
}

function normalizeHumanReviewEvent(ev: AuditEvent): GuardrailRecord | null {
  const eventType = typeof ev.eventType === "string" ? ev.eventType : "";
  if (eventType !== "guardrail.human.review") return null;

  const timestamp = typeof ev.timestamp === "string" ? ev.timestamp : "";
  const reviewedEventId = typeof ev.reviewedEventId === "string" ? ev.reviewedEventId : "";
  const originalDecision = coerceDecision(ev.originalDecision);
  const humanDecision =
    ev.humanDecision === "approve" || ev.humanDecision === "reject"
      ? (ev.humanDecision as "approve" | "reject")
      : null;

  if (!timestamp || !reviewedEventId || !originalDecision || !humanDecision) return null;

  const tool = typeof ev.tool === "string" ? ev.tool : "";
  const summary = typeof ev.summary === "string" ? ev.summary : tool ? `${tool} action` : "review";
  const reason = typeof ev.note === "string" ? ev.note : undefined;

  return {
    id: reviewedEventId,
    timestamp,
    railType: "action",
    inputSummary: redactSummary(summary),
    guardrailDecision: originalDecision,
    humanDecision,
    humanLabel: computeHumanLabel({ guardrailDecision: originalDecision, humanDecision }),
    failureCategory: tagFailureCategory({
      railType: "action",
      inputSummary: summary,
      tool,
      args: undefined,
      reason,
    }),
    meta: {
      sourceEventType: eventType,
      tool,
      reviewer: ev.reviewer,
      note: ev.note,
      label: ev.label,
      riskLevel: ev.riskLevel,
    },
  };
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));
  const cfg = loadConfig();
  const audit = resolveGuardrailsAuditConfig({ guardrails: cfg.guardrails }, STATE_DIR);

  const inputPath = (args.input?.trim() ? args.input.trim() : audit.filePath).trim();
  if (!inputPath) {
    throw new Error(
      "No input path found (guardrails.audit.filePath is empty and --input not provided).",
    );
  }
  if (!fs.existsSync(inputPath)) {
    throw new Error(`Input audit log not found: ${inputPath}`);
  }

  const outDir = path.join(process.cwd(), "data", "guardrails", "normalized");
  ensureDir(outDir);
  const outputPath = args.output?.trim() || path.join(outDir, `guardrails-${nowStamp()}.jsonl`);

  const raw = fs.readFileSync(inputPath, "utf-8");
  const lines = raw.split("\n").filter((l) => l.trim().length > 0);

  const records: GuardrailRecord[] = [];
  let skipped = 0;

  for (const line of lines) {
    const parsed = safeJsonParse(line);
    if (!parsed || typeof parsed !== "object") {
      skipped++;
      continue;
    }
    const ev = parsed as AuditEvent;
    const human = normalizeHumanReviewEvent(ev);
    if (human) {
      records.push(human);
      continue;
    }
    const rec = normalizeGuardrailEvent(ev);
    if (rec) {
      records.push(rec);
      continue;
    }
    skipped++;
  }

  // Write JSONL
  const out = records.map((r) => JSON.stringify(r)).join("\n") + "\n";
  fs.writeFileSync(outputPath, out, "utf-8");

  console.log("Guardrails data export complete.");
  console.log(`- input: ${inputPath}`);
  console.log(`- output: ${outputPath}`);
  console.log(`- records: ${records.length}`);
  console.log(`- skipped: ${skipped}`);
}

main().catch((err) => {
  console.error(`guardrails:data:export failed: ${(err as Error)?.message ?? String(err)}`);
  process.exit(1);
});
