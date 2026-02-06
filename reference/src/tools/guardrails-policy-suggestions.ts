#!/usr/bin/env node
/**
 * OXCER: Generate policy tuning suggestions from normalized guardrails datasets.
 * Sprint 11: Policy tuning loop (data → policy targets → tuning plan).
 *
 * Usage:
 *   pnpm guardrails:policy:suggestions
 *   pnpm guardrails:policy:suggestions --input data/guardrails/normalized --report data/guardrails/reports/guardrails-report-YYYYMMDD.json
 *   pnpm guardrails:policy:suggestions --input data/guardrails/normalized/guardrails-*.jsonl
 */

import fs from "node:fs";
import path from "node:path";
import process from "node:process";

type RailType = "input" | "action" | "result";
type GuardrailDecision = "allow" | "deny" | "needs_human";
type HumanLabel = "tp" | "tn" | "fp" | "fn";

type GuardrailRecord = {
  id: string;
  timestamp: string;
  railType: RailType;
  inputSummary: string;
  guardrailDecision: GuardrailDecision;
  humanDecision?: "approve" | "reject";
  humanLabel?: HumanLabel;
  failureCategory?: string;
  meta?: Record<string, unknown>;
};

export type PolicySuggestion = {
  category: string; // failureCategory (or "unknown")
  railType: RailType;
  issue: "high_fp" | "high_fn" | "high_needs_human";
  sampleCount: number;
  fp: number;
  fn: number;
  needsHuman: number;
  exampleIds: string[];
};

function parseArgs(argv: string[]) {
  const out: { input?: string; report?: string; output?: string } = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--input") {
      out.input = argv[i + 1];
      i++;
      continue;
    }
    if (a === "--report") {
      out.report = argv[i + 1];
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

function ensureDir(p: string): void {
  fs.mkdirSync(p, { recursive: true, mode: 0o755 });
}

function nowStamp(): string {
  // YYYYMMDD
  const d = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}`;
}

function safeJsonParse(line: string): unknown {
  try {
    return JSON.parse(line);
  } catch {
    return null;
  }
}

function readJsonl(filePath: string): GuardrailRecord[] {
  const raw = fs.readFileSync(filePath, "utf-8");
  const lines = raw.split("\n").filter((l) => l.trim().length > 0);
  const out: GuardrailRecord[] = [];
  for (const line of lines) {
    const parsed = safeJsonParse(line);
    if (!parsed || typeof parsed !== "object") continue;
    const rec = parsed as GuardrailRecord;
    if (!rec.id || !rec.timestamp || !rec.guardrailDecision || !rec.railType) continue;
    out.push(rec);
  }
  return out;
}

function padRight(s: string, n: number): string {
  if (s.length >= n) return s;
  return s + " ".repeat(n - s.length);
}

function formatPercent(n: number): string {
  return `${(n * 100).toFixed(1)}%`;
}

function parseOptionalReport(reportPath: string | undefined): unknown | null {
  if (!reportPath?.trim()) return null;
  const p = reportPath.trim();
  if (!fs.existsSync(p)) return null;
  try {
    return JSON.parse(fs.readFileSync(p, "utf-8"));
  } catch {
    return null;
  }
}

function pickExampleIds(params: {
  records: GuardrailRecord[];
  issue: PolicySuggestion["issue"];
  max: number;
}): string[] {
  const max = Math.max(0, params.max);
  const out: string[] = [];
  for (const r of params.records) {
    if (out.length >= max) break;
    if (params.issue === "high_fp" && r.humanLabel === "fp") out.push(r.id);
    else if (params.issue === "high_fn" && r.humanLabel === "fn") out.push(r.id);
    else if (params.issue === "high_needs_human" && r.guardrailDecision === "needs_human")
      out.push(r.id);
  }
  return out;
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));
  const defaultInput = path.join(process.cwd(), "data", "guardrails", "normalized");
  const inputPath = args.input?.trim() || defaultInput;

  // Optional (currently unused for scoring, but accepted for future iterations)
  void parseOptionalReport(args.report);

  let files: string[] = [];
  if (fs.existsSync(inputPath) && fs.statSync(inputPath).isDirectory()) {
    files = fs
      .readdirSync(inputPath)
      .filter((f) => f.endsWith(".jsonl"))
      .map((f) => path.join(inputPath, f));
  } else {
    files = [inputPath];
  }

  if (files.length === 0) {
    throw new Error(`No JSONL files found under: ${inputPath}`);
  }

  const records = files.flatMap((f) => readJsonl(f));
  const total = records.length;

  type GroupKey = string;
  const keyOf = (cat: string, railType: RailType): GroupKey => `${cat}::${railType}`;

  const groups = new Map<GroupKey, GuardrailRecord[]>();
  for (const r of records) {
    const cat = (r.failureCategory?.trim() ? r.failureCategory.trim() : "unknown") || "unknown";
    const k = keyOf(cat, r.railType);
    const arr = groups.get(k) ?? [];
    arr.push(r);
    groups.set(k, arr);
  }

  // Thresholds (intentionally simple for Sprint 11 scaffolding)
  const minLabeledForFpFn = 8;
  const minTotalForNeedsHuman = 12;
  const highFpRate = 0.25;
  const highFnRate = 0.15;
  const highNeedsHumanRate = 0.3;

  const suggestions: PolicySuggestion[] = [];

  for (const [k, rs] of groups.entries()) {
    const [category, railTypeRaw] = k.split("::");
    const railType = railTypeRaw as RailType;

    let fp = 0;
    let fn = 0;
    let tp = 0;
    let tn = 0;
    let needsHuman = 0;
    let labeled = 0;

    for (const r of rs) {
      if (r.guardrailDecision === "needs_human") needsHuman++;
      if (r.humanLabel) {
        labeled++;
        if (r.humanLabel === "fp") fp++;
        else if (r.humanLabel === "fn") fn++;
        else if (r.humanLabel === "tp") tp++;
        else if (r.humanLabel === "tn") tn++;
      }
    }

    const fpRate = labeled > 0 ? fp / labeled : 0;
    const fnRate = labeled > 0 ? fn / labeled : 0;
    const needsHumanRate = rs.length > 0 ? needsHuman / rs.length : 0;

    if (labeled >= minLabeledForFpFn && fpRate >= highFpRate && fp >= 2) {
      suggestions.push({
        category,
        railType,
        issue: "high_fp",
        sampleCount: rs.length,
        fp,
        fn,
        needsHuman,
        exampleIds: pickExampleIds({ records: rs, issue: "high_fp", max: 6 }),
      });
    }

    if (labeled >= minLabeledForFpFn && fnRate >= highFnRate && fn >= 2) {
      suggestions.push({
        category,
        railType,
        issue: "high_fn",
        sampleCount: rs.length,
        fp,
        fn,
        needsHuman,
        exampleIds: pickExampleIds({ records: rs, issue: "high_fn", max: 6 }),
      });
    }

    if (
      rs.length >= minTotalForNeedsHuman &&
      needsHumanRate >= highNeedsHumanRate &&
      needsHuman >= 3
    ) {
      suggestions.push({
        category,
        railType,
        issue: "high_needs_human",
        sampleCount: rs.length,
        fp,
        fn,
        needsHuman,
        exampleIds: pickExampleIds({ records: rs, issue: "high_needs_human", max: 6 }),
      });
    }
  }

  suggestions.sort((a, b) => {
    const score = (s: PolicySuggestion) => {
      if (s.issue === "high_fn") return 300000 + s.fn * 1000 + s.sampleCount;
      if (s.issue === "high_fp") return 200000 + s.fp * 1000 + s.sampleCount;
      return 100000 + s.needsHuman * 1000 + s.sampleCount;
    };
    return score(b) - score(a);
  });

  console.log("Guardrails policy suggestions");
  console.log(`- inputs: ${files.length} file(s)`);
  console.log(`- records: ${total}`);
  console.log("");

  if (suggestions.length === 0) {
    console.log("No tuning candidates found (thresholds not met).");
  } else {
    console.log(
      `${padRight("category", 24)} ${padRight("rail", 8)} ${padRight("issue", 18)} ${padRight("n", 6)} ${padRight("fp", 6)} ${padRight("fn", 6)} ${padRight("needs_h", 10)}`,
    );
    for (const s of suggestions.slice(0, 30)) {
      console.log(
        `${padRight(s.category, 24)} ${padRight(s.railType, 8)} ${padRight(s.issue, 18)} ${padRight(String(s.sampleCount), 6)} ${padRight(String(s.fp), 6)} ${padRight(String(s.fn), 6)} ${padRight(String(s.needsHuman), 10)}`,
      );
    }
    if (suggestions.length > 30) {
      console.log(`… and ${suggestions.length - 30} more`);
    }
  }

  const reportsDir = path.join(process.cwd(), "data", "guardrails", "reports");
  ensureDir(reportsDir);
  const outputPath =
    args.output?.trim() || path.join(reportsDir, `policy-suggestions-${nowStamp()}.json`);

  const payload = {
    generatedAt: new Date().toISOString(),
    inputs: files,
    thresholds: {
      minLabeledForFpFn,
      minTotalForNeedsHuman,
      highFpRate,
      highFnRate,
      highNeedsHumanRate,
    },
    suggestions,
  };

  fs.writeFileSync(outputPath, JSON.stringify(payload, null, 2) + "\n", "utf-8");
  console.log("");
  console.log(`Wrote suggestions: ${outputPath}`);
}

main().catch((err) => {
  console.error(`guardrails:policy:suggestions failed: ${(err as Error)?.message ?? String(err)}`);
  process.exit(1);
});
