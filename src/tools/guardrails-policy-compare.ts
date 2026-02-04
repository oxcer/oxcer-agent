#!/usr/bin/env node
/**
 * OXCER: Compare two guardrails policy profiles on the same dataset.
 * Sprint 11: Policy tuning loop (before/after evaluation).
 *
 * Supported datasets:
 * - NeMo eval dataset format (Sprint 8): JSONL with {"type":"action"|"result", ...}
 *   → Runs both configs through the orchestrator and compares decisions.
 * - Normalized dataset format (Sprint 10): JSONL with {railType, guardrailDecision, humanLabel, ...}
 *   → "Replay" mode only: compares distributions from existing decisions (no re-eval).
 *
 * Usage:
 *   pnpm guardrails:policy:compare --configA path/to/old-config.json --configB path/to/new-config.json --dataset test/guardrails/nemo-eval-dataset.jsonl
 *   pnpm guardrails:policy:compare --configA ... --configB ... --dataset data/guardrails/normalized/guardrails-*.jsonl
 */

import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import type { OpenClawConfig } from "../config/types.openclaw.js";
import { createGuardrailOrchestrator } from "../guardrails/orchestrator.js";
import type { ProposedAction } from "../guardrails/orchestrator.js";

type GuardrailDecision = "allow" | "deny" | "needs_human";
type HumanLabel = "tp" | "tn" | "fp" | "fn";
type RailType = "input" | "action" | "result";

type NemoEvalTestCase = {
  type: "action" | "result";
  tool: string;
  args: Record<string, unknown>;
  summary?: string;
  riskLevel?: "low" | "medium" | "high" | "critical";
  result?: unknown;
  context?: Record<string, unknown>;
};

type NormalizedRecord = {
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

type CompareArgs = {
  configA?: string;
  configB?: string;
  dataset?: string;
  output?: string;
};

function parseArgs(argv: string[]): CompareArgs {
  const out: CompareArgs = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--configA") {
      out.configA = argv[i + 1];
      i++;
      continue;
    }
    if (a === "--configB") {
      out.configB = argv[i + 1];
      i++;
      continue;
    }
    if (a === "--dataset") {
      out.dataset = argv[i + 1];
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

function readJsonlLines(filePath: string): unknown[] {
  const raw = fs.readFileSync(filePath, "utf-8");
  const lines = raw.split("\n").filter((l) => l.trim().length > 0);
  const out: unknown[] = [];
  for (const line of lines) {
    const parsed = safeJsonParse(line);
    if (parsed !== null) out.push(parsed);
  }
  return out;
}

function listDatasetFiles(inputPath: string): string[] {
  const trimmed = inputPath.trim();
  if (fs.existsSync(trimmed) && fs.statSync(trimmed).isDirectory()) {
    return fs
      .readdirSync(trimmed)
      .filter((f) => f.endsWith(".jsonl"))
      .map((f) => path.join(trimmed, f));
  }
  return [trimmed];
}

function readConfigFile(p: string): OpenClawConfig {
  if (!fs.existsSync(p)) throw new Error(`Config not found: ${p}`);
  const parsed = JSON.parse(fs.readFileSync(p, "utf-8")) as unknown;
  if (!parsed || typeof parsed !== "object") throw new Error(`Invalid config JSON: ${p}`);
  return parsed as OpenClawConfig;
}

function asDecision(raw: unknown): GuardrailDecision | null {
  if (raw === "allow" || raw === "deny" || raw === "needs_human") return raw;
  return null;
}

function summarizeCounts(decisions: GuardrailDecision[]) {
  const counts: Record<GuardrailDecision, number> = { allow: 0, deny: 0, needs_human: 0 };
  for (const d of decisions) counts[d] = (counts[d] ?? 0) + 1;
  const total = decisions.length;
  const rate = (n: number) => (total > 0 ? n / total : 0);
  return {
    total,
    counts,
    rates: {
      allow: rate(counts.allow),
      deny: rate(counts.deny),
      needs_human: rate(counts.needs_human),
    },
  };
}

function computeFpFnFromNormalized(records: NormalizedRecord[]) {
  let fp = 0;
  let fn = 0;
  let tp = 0;
  let tn = 0;
  let reviewed = 0;
  for (const r of records) {
    if (!r.humanLabel) continue;
    reviewed++;
    if (r.humanLabel === "fp") fp++;
    else if (r.humanLabel === "fn") fn++;
    else if (r.humanLabel === "tp") tp++;
    else if (r.humanLabel === "tn") tn++;
  }
  return { reviewed, fp, fn, tp, tn };
}

function padRight(s: string, n: number): string {
  if (s.length >= n) return s;
  return s + " ".repeat(n - s.length);
}

function formatPercent(n: number): string {
  return `${(n * 100).toFixed(1)}%`;
}

async function compareOnNemoEvalDataset(params: {
  configAPath: string;
  configBPath: string;
  datasetFiles: string[];
}) {
  const cfgA = readConfigFile(params.configAPath);
  const cfgB = readConfigFile(params.configBPath);
  const orchA = createGuardrailOrchestrator(cfgA);
  const orchB = createGuardrailOrchestrator(cfgB);

  const testCases: NemoEvalTestCase[] = params.datasetFiles.flatMap((f) => {
    const lines = readJsonlLines(f);
    const out: NemoEvalTestCase[] = [];
    for (const v of lines) {
      if (!v || typeof v !== "object") continue;
      const tc = v as Partial<NemoEvalTestCase>;
      if (tc.type !== "action" && tc.type !== "result") continue;
      if (typeof tc.tool !== "string" || !tc.tool) continue;
      if (!tc.args || typeof tc.args !== "object") continue;
      out.push(tc as NemoEvalTestCase);
    }
    return out;
  });

  const decisionsA: GuardrailDecision[] = [];
  const decisionsB: GuardrailDecision[] = [];

  type ChangedDecision = {
    index: number;
    type: NemoEvalTestCase["type"];
    tool: string;
    summary?: string;
    decisionA: GuardrailDecision;
    decisionB: GuardrailDecision;
    reasonA?: string;
    reasonB?: string;
  };
  const changed: ChangedDecision[] = [];

  for (let i = 0; i < testCases.length; i++) {
    const tc = testCases[i];
    const action: ProposedAction = {
      id: `compare-${i + 1}`,
      tool: tc.tool,
      args: tc.args,
      summary: tc.summary,
      riskLevel: tc.riskLevel,
      timestamp: Date.now(),
    };

    if (tc.type === "action") {
      const decA = await orchA.checkAction({ action });
      const decB = await orchB.checkAction({ action });
      const dA = asDecision(decA.decision) ?? "needs_human";
      const dB = asDecision(decB.decision) ?? "needs_human";
      decisionsA.push(dA);
      decisionsB.push(dB);
      if (dA !== dB && changed.length < 12) {
        changed.push({
          index: i,
          type: "action",
          tool: tc.tool,
          summary: tc.summary,
          decisionA: dA,
          decisionB: dB,
          reasonA:
            "reason" in decA
              ? typeof decA.reason === "string"
                ? decA.reason
                : undefined
              : undefined,
          reasonB:
            "reason" in decB
              ? typeof decB.reason === "string"
                ? decB.reason
                : undefined
              : undefined,
        });
      }
      continue;
    }

    const decA = await orchA.checkResult({
      tool: tc.tool,
      args: tc.args,
      result: tc.result,
      context: tc.context,
    });
    const decB = await orchB.checkResult({
      tool: tc.tool,
      args: tc.args,
      result: tc.result,
      context: tc.context,
    });
    const dA = asDecision(decA.decision) ?? "needs_human";
    const dB = asDecision(decB.decision) ?? "needs_human";
    decisionsA.push(dA);
    decisionsB.push(dB);
    if (dA !== dB && changed.length < 12) {
      changed.push({
        index: i,
        type: "result",
        tool: tc.tool,
        summary: tc.summary,
        decisionA: dA,
        decisionB: dB,
        reasonA:
          "reason" in decA
            ? typeof decA.reason === "string"
              ? decA.reason
              : undefined
            : undefined,
        reasonB:
          "reason" in decB
            ? typeof decB.reason === "string"
              ? decB.reason
              : undefined
            : undefined,
      });
    }
  }

  return {
    mode: "nemo-eval" as const,
    testCases: testCases.length,
    summaryA: summarizeCounts(decisionsA),
    summaryB: summarizeCounts(decisionsB),
    changedDecisionsSample: changed,
  };
}

function compareOnNormalizedReplay(params: { datasetFiles: string[] }) {
  const records: NormalizedRecord[] = params.datasetFiles.flatMap((f) => {
    const lines = readJsonlLines(f);
    const out: NormalizedRecord[] = [];
    for (const v of lines) {
      if (!v || typeof v !== "object") continue;
      const r = v as Partial<NormalizedRecord>;
      if (!r.id || !r.timestamp || !r.railType) continue;
      if (!r.guardrailDecision) continue;
      if (asDecision(r.guardrailDecision) === null) continue;
      out.push(r as NormalizedRecord);
    }
    return out;
  });

  const decisions = records.map((r) => r.guardrailDecision);
  const summary = summarizeCounts(decisions);
  const fpfn = computeFpFnFromNormalized(records);

  const perCategory = new Map<
    string,
    { total: number; fp: number; fn: number; needsHuman: number }
  >();
  for (const r of records) {
    const cat = (r.failureCategory?.trim() ? r.failureCategory.trim() : "unknown") || "unknown";
    const entry = perCategory.get(cat) ?? { total: 0, fp: 0, fn: 0, needsHuman: 0 };
    entry.total++;
    if (r.guardrailDecision === "needs_human") entry.needsHuman++;
    if (r.humanLabel === "fp") entry.fp++;
    if (r.humanLabel === "fn") entry.fn++;
    perCategory.set(cat, entry);
  }

  const perCategoryTop = Array.from(perCategory.entries())
    .map(([category, v]) => ({ category, ...v }))
    .sort((a, b) => b.fp + b.fn + b.needsHuman - (a.fp + a.fn + a.needsHuman))
    .slice(0, 20);

  return {
    mode: "normalized-replay" as const,
    records: records.length,
    summary,
    human: fpfn,
    perCategoryTop,
  };
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));
  const configAPath = args.configA?.trim();
  const configBPath = args.configB?.trim();
  const datasetPath = args.dataset?.trim();

  if (!datasetPath) {
    throw new Error("Missing required --dataset (JSONL file or directory).");
  }

  const datasetFiles = listDatasetFiles(datasetPath).filter((p) => p.endsWith(".jsonl"));
  if (datasetFiles.length === 0) {
    throw new Error(`No JSONL dataset files found under: ${datasetPath}`);
  }

  // Detect dataset type by peeking at the first non-empty line of the first file.
  const first = readJsonlLines(datasetFiles[0])[0];
  const isNemoEval = !!(first && typeof first === "object" && (first as { type?: unknown }).type);

  const generatedAt = new Date().toISOString();
  const reportsDir = path.join(process.cwd(), "data", "guardrails", "reports");
  ensureDir(reportsDir);
  const outputPath =
    args.output?.trim() || path.join(reportsDir, `policy-compare-${nowStamp()}.json`);

  let report: Record<string, unknown>;

  if (isNemoEval) {
    if (!configAPath || !configBPath) {
      throw new Error(
        "For NeMo eval datasets, both --configA and --configB are required (paths to OpenClaw config JSON).",
      );
    }

    const result = await compareOnNemoEvalDataset({ configAPath, configBPath, datasetFiles });
    report = {
      generatedAt,
      mode: result.mode,
      dataset: datasetFiles,
      configA: configAPath,
      configB: configBPath,
      results: result,
    };

    // Print concise summary
    console.log("Guardrails policy compare (NeMo eval dataset)");
    console.log(`- dataset files: ${datasetFiles.length}`);
    console.log(`- test cases: ${result.testCases}`);
    console.log("");
    console.log("Decision distribution");
    console.log(
      `${padRight("", 14)} ${padRight("allow", 10)} ${padRight("deny", 10)} ${padRight("needs_human", 12)}`,
    );
    console.log(
      `${padRight("policy A", 14)} ${padRight(`${result.summaryA.counts.allow} (${formatPercent(result.summaryA.rates.allow)})`, 10)} ${padRight(`${result.summaryA.counts.deny} (${formatPercent(result.summaryA.rates.deny)})`, 10)} ${padRight(`${result.summaryA.counts.needs_human} (${formatPercent(result.summaryA.rates.needs_human)})`, 12)}`,
    );
    console.log(
      `${padRight("policy B", 14)} ${padRight(`${result.summaryB.counts.allow} (${formatPercent(result.summaryB.rates.allow)})`, 10)} ${padRight(`${result.summaryB.counts.deny} (${formatPercent(result.summaryB.rates.deny)})`, 10)} ${padRight(`${result.summaryB.counts.needs_human} (${formatPercent(result.summaryB.rates.needs_human)})`, 12)}`,
    );
    console.log("");
    if (result.changedDecisionsSample.length > 0) {
      console.log("Changed decisions (sample)");
      for (const c of result.changedDecisionsSample) {
        console.log(
          `- [${c.index + 1}] ${c.type} ${c.tool}: A=${c.decisionA} B=${c.decisionB}${c.summary ? ` — ${c.summary}` : ""}`,
        );
      }
      console.log("");
    }
  } else {
    const result = compareOnNormalizedReplay({ datasetFiles });
    report = {
      generatedAt,
      mode: result.mode,
      dataset: datasetFiles,
      note: "Replay mode only for normalized datasets (no re-evaluation). Use a NeMo eval dataset to run A/B policy checks.",
      results: result,
    };

    console.log("Guardrails policy compare (normalized replay)");
    console.log(`- dataset files: ${datasetFiles.length}`);
    console.log(`- records: ${result.records}`);
    console.log("");
    console.log("Decision distribution (observed)");
    console.log(
      `${padRight("allow", 12)} ${padRight(String(result.summary.counts.allow), 8)} ${formatPercent(result.summary.rates.allow)}`,
    );
    console.log(
      `${padRight("deny", 12)} ${padRight(String(result.summary.counts.deny), 8)} ${formatPercent(result.summary.rates.deny)}`,
    );
    console.log(
      `${padRight("needs_human", 12)} ${padRight(String(result.summary.counts.needs_human), 8)} ${formatPercent(result.summary.rates.needs_human)}`,
    );
    console.log("");
    if (result.human.reviewed > 0) {
      console.log("Human labels (where present)");
      console.log(`${padRight("reviewed", 12)} ${result.human.reviewed}`);
      console.log(`${padRight("fp", 12)} ${result.human.fp}`);
      console.log(`${padRight("fn", 12)} ${result.human.fn}`);
      console.log("");
    }
  }

  fs.writeFileSync(outputPath, JSON.stringify(report, null, 2) + "\n", "utf-8");
  console.log(`Wrote compare report: ${outputPath}`);
}

main().catch((err) => {
  console.error(`guardrails:policy:compare failed: ${(err as Error)?.message ?? String(err)}`);
  process.exit(1);
});
