#!/usr/bin/env node
/**
 * OXCER: Basic metrics/report for normalized guardrails datasets.
 * Sprint 10: Safety metrics + HITL agreement reporting.
 *
 * Usage:
 *   pnpm guardrails:data:report
 *   pnpm guardrails:data:report --input data/guardrails/normalized/guardrails-*.jsonl --output data/guardrails/reports/report-YYYYMMDD.json
 */

import fs from "node:fs";
import path from "node:path";
import process from "node:process";

type GuardrailRecord = {
  id: string;
  timestamp: string;
  railType: "input" | "action" | "result";
  inputSummary: string;
  guardrailDecision: "allow" | "deny" | "needs_human";
  humanDecision?: "approve" | "reject";
  humanLabel?: "tp" | "tn" | "fp" | "fn";
  failureCategory?: string;
  meta?: Record<string, unknown>;
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

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));
  const defaultInput = path.join(process.cwd(), "data", "guardrails", "normalized");
  const inputPath = args.input?.trim() || defaultInput;

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

  const byDecision = {
    allow: 0,
    deny: 0,
    needs_human: 0,
  } as const;
  const countsByDecision: Record<keyof typeof byDecision, number> = {
    allow: 0,
    deny: 0,
    needs_human: 0,
  };

  const withHuman = records.filter((r) => r.humanDecision);
  const agreement = {
    total: withHuman.length,
    agree: 0,
    fp: 0,
    fn: 0,
    tp: 0,
    tn: 0,
  };

  const failureCategoryCounts = new Map<string, number>();

  for (const r of records) {
    countsByDecision[r.guardrailDecision] = (countsByDecision[r.guardrailDecision] ?? 0) + 1;
    if (r.failureCategory) {
      failureCategoryCounts.set(
        r.failureCategory,
        (failureCategoryCounts.get(r.failureCategory) ?? 0) + 1,
      );
    }
    if (r.humanDecision) {
      if (r.humanLabel === "fp") agreement.fp++;
      else if (r.humanLabel === "fn") agreement.fn++;
      else if (r.humanLabel === "tp") agreement.tp++;
      else if (r.humanLabel === "tn") agreement.tn++;

      // agreement heuristic:
      // - allow vs approve => agree
      // - deny vs reject => agree
      // - needs_human always counts as agree (escalation), since it defers to human
      const g = r.guardrailDecision;
      const h = r.humanDecision;
      const agree =
        g === "needs_human" ||
        (g === "allow" && h === "approve") ||
        (g === "deny" && h === "reject");
      if (agree) agreement.agree++;
    }
  }

  const blockRate = total > 0 ? countsByDecision.deny / total : 0;
  const needsHumanRate = total > 0 ? countsByDecision.needs_human / total : 0;
  const allowRate = total > 0 ? countsByDecision.allow / total : 0;
  const agreementRate = agreement.total > 0 ? agreement.agree / agreement.total : 0;

  // Print concise table
  console.log("Guardrails data report");
  console.log(`- inputs: ${files.length} file(s)`);
  console.log(`- records: ${total}`);
  console.log("");
  console.log("Decision distribution");
  console.log(
    `${padRight("allow", 12)} ${padRight(String(countsByDecision.allow), 8)} ${formatPercent(allowRate)}`,
  );
  console.log(
    `${padRight("deny", 12)} ${padRight(String(countsByDecision.deny), 8)} ${formatPercent(blockRate)}`,
  );
  console.log(
    `${padRight("needs_human", 12)} ${padRight(String(countsByDecision.needs_human), 8)} ${formatPercent(needsHumanRate)}`,
  );
  console.log("");
  console.log("Human review agreement (where humanDecision present)");
  console.log(`${padRight("total reviewed", 16)} ${agreement.total}`);
  console.log(`${padRight("agreement", 16)} ${agreement.agree} (${formatPercent(agreementRate)})`);
  console.log(`${padRight("fp", 16)} ${agreement.fp}`);
  console.log(`${padRight("fn", 16)} ${agreement.fn}`);
  console.log("");

  const topFailure = Array.from(failureCategoryCounts.entries())
    .sort((a, b) => b[1] - a[1])
    .slice(0, 10);
  if (topFailure.length > 0) {
    console.log("Top failureCategory counts");
    for (const [cat, n] of topFailure) {
      console.log(`${padRight(cat, 24)} ${n}`);
    }
    console.log("");
  }

  const report = {
    generatedAt: new Date().toISOString(),
    inputs: files,
    totals: {
      records: total,
      allow: countsByDecision.allow,
      deny: countsByDecision.deny,
      needs_human: countsByDecision.needs_human,
      blockRate,
      needsHumanRate,
      allowRate,
    },
    human: {
      reviewed: agreement.total,
      agreement: agreement.agree,
      agreementRate,
      fp: agreement.fp,
      fn: agreement.fn,
      tp: agreement.tp,
      tn: agreement.tn,
    },
    failureCategoryTop: topFailure,
  };

  const reportsDir = path.join(process.cwd(), "data", "guardrails", "reports");
  ensureDir(reportsDir);
  const outputPath =
    args.output?.trim() || path.join(reportsDir, `guardrails-report-${nowStamp()}.json`);
  fs.writeFileSync(outputPath, JSON.stringify(report, null, 2) + "\n", "utf-8");
  console.log(`Wrote report: ${outputPath}`);
}

main().catch((err) => {
  console.error(`guardrails:data:report failed: ${(err as Error)?.message ?? String(err)}`);
  process.exit(1);
});
