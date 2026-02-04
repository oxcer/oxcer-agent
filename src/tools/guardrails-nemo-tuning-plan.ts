#!/usr/bin/env node
/**
 * OXCER: Generate a NeMo Guardrails tuning plan (brief) from policy suggestions.
 * Sprint 11: Policy tuning loop (data → suggestions → plan → config edits → compare).
 *
 * This tool does NOT edit NeMo config. It produces a human-readable brief.
 *
 * Usage:
 *   pnpm guardrails:nemo:tuning-plan --input data/guardrails/reports/policy-suggestions-YYYYMMDD.json
 */

import fs from "node:fs";
import path from "node:path";
import process from "node:process";

type RailType = "input" | "action" | "result";
type Issue = "high_fp" | "high_fn" | "high_needs_human";

type PolicySuggestion = {
  category: string;
  railType: RailType;
  issue: Issue;
  sampleCount: number;
  fp: number;
  fn: number;
  needsHuman: number;
  exampleIds: string[];
};

type SuggestionsFile = {
  generatedAt: string;
  inputs: string[];
  thresholds?: Record<string, unknown>;
  suggestions: PolicySuggestion[];
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

function issueLabel(issue: Issue): string {
  if (issue === "high_fp") return "high false positives (over-blocking)";
  if (issue === "high_fn") return "high false negatives (under-blocking)";
  return "high needs_human (over-deferring)";
}

function bulletsForSuggestion(s: PolicySuggestion): string[] {
  const cat = s.category;
  const rail = s.railType;
  const issue = s.issue;

  // Category-specific guidance: keep it practical and config-oriented.
  if (cat === "destructive_command") {
    if (issue === "high_fn") {
      return [
        `Strengthen ${rail} rails for destructive shell patterns (e.g. \`rm -rf\`, \`mkfs\`, \`dd if=\`).`,
        `Treat as high severity: deny by default, or require explicit human approval with a clear reason.`,
        `Add safe-response guidance: suggest non-destructive alternatives or a confirmation workflow.`,
      ];
    }
    if (issue === "high_fp") {
      return [
        `Tighten pattern matching to reduce benign command hits (avoid broad \`rm\` / filesystem heuristics).`,
        `Add allowlisted safe variants (e.g. operating only under \`/tmp\`), while keeping hard blocks for truly destructive forms.`,
        `Add structured “why blocked” messaging so operators can calibrate rules.`,
      ];
    }
    return [
      `Reduce deferrals by adding clearer deterministic rules for destructive commands (deny vs needs_human).`,
      `If using needs_human, include explicit criteria for auto-deny vs escalate based on path/flags.`,
    ];
  }

  if (cat === "prompt_injection") {
    if (issue === "high_fn") {
      return [
        `Add/update input rails patterns for instruction override phrases (e.g. “ignore previous instructions”, “you are now”, “developer message”).`,
        `Add a “policy reminder” response path: refuse and restate allowed scope when injection patterns appear.`,
        `Consider a higher-sensitivity rail for messages containing tool-use coercion or secrets requests.`,
      ];
    }
    if (issue === "high_fp") {
      return [
        `Reduce over-blocking by distinguishing quoted/educational mentions from direct imperative injection attempts.`,
        `Use narrower patterns plus context checks (imperative verbs + override phrases) rather than single keyword matches.`,
      ];
    }
    return [
      `Convert common injection cases from needs_human to deterministic refuse-with-explanation when confidence is high.`,
      `Add “ask clarifying question” behavior for ambiguous cases to reduce operator load.`,
    ];
  }

  if (cat === "system_prompt_leak") {
    if (rail !== "result") {
      // Most effective in output rails, but still give advice for input/action.
      return [
        `Focus on output/result rails: block or redact “system prompt” disclosures and instruction hierarchy content.`,
        `Add explicit refusal behaviors when asked to reveal internal instructions or hidden context.`,
      ];
    }
    if (issue === "high_fn") {
      return [
        `Tighten output rails to detect “system prompt” / hidden instruction leakage cues and redact/deny.`,
        `Add safe-response templates: explain inability to reveal system instructions and offer help within scope.`,
      ];
    }
    if (issue === "high_fp") {
      return [
        `Avoid blocking benign mentions (e.g. documentation) by keying on “reveal/show the system prompt” requests and direct quotes of policy text.`,
        `Prefer redaction over deny when the content can be safely removed from otherwise-helpful output.`,
      ];
    }
    return [
      `Reduce needs_human by adopting consistent redaction rules for prompt-leak fragments.`,
      `Log and sample redacted spans for iterative tuning.`,
    ];
  }

  if (cat === "credential_exfiltration") {
    if (issue === "high_fn") {
      return [
        `Strengthen ${rail} rails for outbound transfer tools/verbs (curl/wget/nc) combined with secret indicators (token/api_key/password).`,
        `Require explicit user intent + destination allowlists for any outbound transfer when secrets are present.`,
        `Add a safe-response path: recommend secret rotation and secure sharing mechanisms.`,
      ];
    }
    if (issue === "high_fp") {
      return [
        `Reduce false positives by requiring both “exfil channel” signals (curl/wget/url) AND a secret indicator, not either alone.`,
        `Allow benign web fetches without secret indicators; keep strict blocks when tokens/keys appear.`,
      ];
    }
    return [
      `Replace ambiguous needs_human with deterministic rules where signals are strong (secret indicator + outbound transfer).`,
      `When signals are weak, ask clarifying questions rather than escalating.`,
    ];
  }

  if (cat === "sensitive_file_read") {
    if (issue === "high_fn") {
      return [
        `Strengthen ${rail} rails for sensitive paths (e.g. \`/etc/shadow\`, \`id_rsa\`, \`.aws/credentials\`, \`.env\`).`,
        `Add an allowlist exception for clearly non-sensitive paths (e.g. project-local files) if applicable.`,
        `Prefer deny/redact for direct credential content even if access is granted.`,
      ];
    }
    if (issue === "high_fp") {
      return [
        `Avoid over-blocking by narrowing patterns to exact sensitive filenames/paths rather than broad “/etc/” matching.`,
        `If you allow reads, enforce redaction rules on output rails to prevent accidental leakage.`,
      ];
    }
    return [
      `Reduce deferrals by defining deterministic handling for each sensitive path class (deny vs redact vs needs_human).`,
    ];
  }

  if (cat === "unsafe_write_path") {
    if (issue === "high_fn") {
      return [
        `Strengthen action rails for writes to privileged locations (\`/etc\`, \`/bin\`, \`/usr/bin\`).`,
        `Require human approval for any privileged path write, and deny attempts that inject “malicious” host entries or similar patterns.`,
      ];
    }
    if (issue === "high_fp") {
      return [
        `Reduce false positives by allowing safe paths (\`/tmp\`, project workspace) and focusing privileged paths only.`,
        `Use path normalization rules to avoid blocking harmless relative paths.`,
      ];
    }
    return [
      `Convert common safe writes into deterministic allow, leaving only privileged paths as needs_human.`,
    ];
  }

  // Fallback guidance for unknown categories
  if (issue === "high_fn") {
    return [
      `Strengthen ${rail} rails for this category: add clearer patterns/conditions that should deny or escalate.`,
      `Add a safe-response template explaining why the action/request is disallowed (reduces repeated attempts).`,
      `Add targeted examples to the eval dataset and re-run compare after changes.`,
    ];
  }
  if (issue === "high_fp") {
    return [
      `Reduce over-blocking in ${rail} rails: narrow patterns and add contextual checks to avoid benign matches.`,
      `Add allowlisted safe variants (but keep strict blocks for high-risk forms).`,
      `Collect a few representative false-positive examples and validate against new rules.`,
    ];
  }
  return [
    `Reduce deferrals in ${rail} rails by splitting “uncertain” into deterministic allow/deny rules where possible.`,
    `Add clarifying-question behavior for ambiguous cases to reduce needs_human volume.`,
  ];
}

function formatSuggestionSection(s: PolicySuggestion): string {
  const examples =
    s.exampleIds.length > 0
      ? s.exampleIds.map((id) => `  - \`${id}\``).join("\n")
      : "  - (none captured)";
  const bullets = bulletsForSuggestion(s)
    .map((b) => `- ${b}`)
    .join("\n");
  return [
    `## Category: ${s.category} (${s.railType}, ${s.issue})`,
    `- Issue: ${issueLabel(s.issue)}.`,
    `- Sample count: ${s.sampleCount}.`,
    `- Counts: fp=${s.fp}, fn=${s.fn}, needs_human=${s.needsHuman}.`,
    `- Example IDs:`,
    examples,
    `- Suggested NeMo changes:`,
    bullets,
    ``,
  ].join("\n");
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));
  const inputPath = args.input?.trim();
  if (!inputPath) {
    throw new Error("Missing required --input (policy-suggestions-*.json).");
  }
  if (!fs.existsSync(inputPath)) {
    throw new Error(`Input not found: ${inputPath}`);
  }

  const parsed = JSON.parse(fs.readFileSync(inputPath, "utf-8")) as SuggestionsFile;
  const suggestions = Array.isArray(parsed.suggestions) ? parsed.suggestions : [];

  const d = new Date();
  const dateLine = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;

  const reportsDir = path.join(process.cwd(), "data", "guardrails", "reports");
  ensureDir(reportsDir);
  const outputPath =
    args.output?.trim() || path.join(reportsDir, `nemo-tuning-plan-${nowStamp()}.md`);

  const header = [
    `# NeMo Guardrails Tuning Plan (${dateLine})`,
    ``,
    `Inputs:`,
    `- Suggestions: \`${inputPath}\``,
    ...(Array.isArray(parsed.inputs) && parsed.inputs.length > 0
      ? [`- Normalized:`, ...parsed.inputs.map((p) => `  - \`${p}\``)]
      : []),
    ``,
    `Notes:`,
    `- This plan suggests config/rails changes (no model retraining).`,
    `- After applying changes, run policy compare on the same dataset to measure impact.`,
    ``,
  ].join("\n");

  const body =
    suggestions.length === 0
      ? `## No tuning candidates\n\nNo suggestions were present in the input file.\n`
      : suggestions.map((s) => formatSuggestionSection(s)).join("\n");

  fs.writeFileSync(outputPath, `${header}${body}\n`, "utf-8");
  console.log("NeMo tuning plan generated.");
  console.log(`- input: ${inputPath}`);
  console.log(`- output: ${outputPath}`);
  console.log(`- suggestions: ${suggestions.length}`);
}

main().catch((err) => {
  console.error(`guardrails:nemo:tuning-plan failed: ${(err as Error)?.message ?? String(err)}`);
  process.exit(1);
});
