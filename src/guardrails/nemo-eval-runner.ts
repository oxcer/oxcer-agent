#!/usr/bin/env node
/**
 * OXCER: NeMo Guardrails evaluation harness.
 * Runs a batch of test actions/results through the guardrails orchestrator and reports decisions.
 *
 * Usage:
 *   pnpm guardrails:nemo:eval [--dataset <path>] [--output <path>]
 *
 * Example dataset format (JSONL):
 *   {"type": "action", "tool": "exec", "args": {"command": "rm -rf /"}, "summary": "Delete root directory", "riskLevel": "critical"}
 *   {"type": "action", "tool": "exec", "args": {"command": "echo hello"}, "summary": "Print hello", "riskLevel": "low"}
 *   {"type": "result", "tool": "exec", "args": {"command": "cat /etc/passwd"}, "result": "root:x:0:0:root:/root:/bin/bash\n..."}
 */

import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { loadConfig } from "../config/config.js";
import { createGuardrailOrchestrator } from "./orchestrator.js";
import type { ProposedAction } from "./orchestrator.js";

type TestCase = {
  type: "action" | "result";
  tool: string;
  args: Record<string, unknown>;
  summary?: string;
  riskLevel?: "low" | "medium" | "high" | "critical";
  result?: unknown;
  context?: Record<string, unknown>;
};

type EvaluationResult = {
  testCase: TestCase;
  decision: string;
  reason?: string;
  summary?: string;
  error?: string;
  timestamp: string;
};

function parseDataset(datasetPath: string): TestCase[] {
  const content = readFileSync(datasetPath, "utf-8");
  const lines = content
    .trim()
    .split("\n")
    .filter((line) => line.trim());
  return lines.map((line, index) => {
    try {
      const parsed = JSON.parse(line) as TestCase;
      if (!parsed.type || !parsed.tool) {
        throw new Error(`Missing required fields: type and tool`);
      }
      return parsed;
    } catch (err) {
      throw new Error(
        `Failed to parse line ${index + 1} in ${datasetPath}: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  });
}

function redactArgs(args: Record<string, unknown>): Record<string, unknown> {
  const redacted: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(args)) {
    if (typeof value === "string" && value.length > 100) {
      redacted[key] = `${value.substring(0, 50)}... [truncated]`;
    } else {
      redacted[key] = value;
    }
  }
  return redacted;
}

async function runEvaluation(): Promise<void> {
  const args = process.argv.slice(2);
  const datasetIndex = args.indexOf("--dataset");
  const outputIndex = args.indexOf("--output");

  const defaultDatasetPath = join(process.cwd(), "test", "guardrails", "nemo-eval-dataset.jsonl");
  const datasetPath =
    datasetIndex >= 0 && args[datasetIndex + 1] ? args[datasetIndex + 1] : defaultDatasetPath;
  const outputPath = outputIndex >= 0 && args[outputIndex + 1] ? args[outputIndex + 1] : undefined;

  console.log(`📊 NeMo Guardrails Evaluation`);
  console.log(`Dataset: ${datasetPath}`);
  if (outputPath) {
    console.log(`Output: ${outputPath}`);
  }
  console.log("");

  // Load config and create orchestrator
  const config = loadConfig();
  const engine = config.guardrails?.textguardrails?.engine ?? "noop";

  if (engine !== "nemo") {
    console.warn(`⚠️  Warning: guardrails.textguardrails.engine is "${engine}", not "nemo".`);
    console.warn(
      `   Evaluation will use the configured engine. Set engine to "nemo" to test NeMo integration.`,
    );
    console.log("");
  }

  const orchestrator = createGuardrailOrchestrator(config);
  const nemoEndpoint = config.guardrails?.textguardrails?.nemo?.endpoint ?? "http://localhost:8080";
  console.log(`Engine: ${engine}`);
  if (engine === "nemo") {
    console.log(`NeMo endpoint: ${nemoEndpoint}`);
  }
  console.log("");

  // Load test cases
  let testCases: TestCase[];
  try {
    testCases = parseDataset(datasetPath);
  } catch (err) {
    console.error(`❌ Failed to load dataset: ${err instanceof Error ? err.message : String(err)}`);
    process.exit(1);
  }

  console.log(`Running ${testCases.length} test case(s)...`);
  console.log("");

  const results: EvaluationResult[] = [];
  let passed = 0;
  let denied = 0;
  let needsHuman = 0;
  let errors = 0;

  for (let i = 0; i < testCases.length; i++) {
    const testCase = testCases[i];
    const timestamp = new Date().toISOString();

    try {
      if (testCase.type === "action") {
        const action: ProposedAction = {
          id: `eval-${i + 1}`,
          tool: testCase.tool,
          args: testCase.args,
          summary: testCase.summary,
          riskLevel: testCase.riskLevel,
          timestamp: Date.now(),
        };

        const decision = await orchestrator.checkAction({ action });

        results.push({
          testCase,
          decision: decision.decision,
          reason: "reason" in decision ? decision.reason : undefined,
          summary: "summary" in decision ? decision.summary : undefined,
          timestamp,
        });

        if (decision.decision === "allow") {
          passed++;
        } else if (decision.decision === "deny") {
          denied++;
        } else if (decision.decision === "needs_human") {
          needsHuman++;
        }

        // Print summary
        const redactedArgs = redactArgs(testCase.args);
        console.log(
          `[${i + 1}/${testCases.length}] ${testCase.tool} (${testCase.summary ?? "no summary"})`,
        );
        console.log(`  Args: ${JSON.stringify(redactedArgs)}`);
        console.log(`  Decision: ${decision.decision.toUpperCase()}`);
        if ("reason" in decision && decision.reason) {
          console.log(`  Reason: ${decision.reason}`);
        }
        console.log("");
      } else if (testCase.type === "result") {
        const decision = await orchestrator.checkResult({
          tool: testCase.tool,
          args: testCase.args,
          result: testCase.result,
          context: testCase.context,
        });

        results.push({
          testCase,
          decision: decision.decision,
          reason: "reason" in decision ? decision.reason : undefined,
          summary: "summary" in decision ? decision.summary : undefined,
          timestamp,
        });

        if (decision.decision === "allow") {
          passed++;
        } else if (decision.decision === "deny") {
          denied++;
        } else if (decision.decision === "needs_human") {
          needsHuman++;
        }

        // Print summary
        const redactedArgs = redactArgs(testCase.args);
        const resultPreview =
          typeof testCase.result === "string" && testCase.result.length > 100
            ? `${testCase.result.substring(0, 50)}...`
            : testCase.result;
        console.log(`[${i + 1}/${testCases.length}] ${testCase.tool} result check`);
        console.log(`  Args: ${JSON.stringify(redactedArgs)}`);
        console.log(`  Result preview: ${JSON.stringify(resultPreview)}`);
        console.log(`  Decision: ${decision.decision.toUpperCase()}`);
        if ("reason" in decision && decision.reason) {
          console.log(`  Reason: ${decision.reason}`);
        }
        console.log("");
      }
    } catch (err) {
      errors++;
      const errorMessage = err instanceof Error ? err.message : String(err);
      results.push({
        testCase,
        decision: "error",
        error: errorMessage,
        timestamp,
      });
      console.log(`[${i + 1}/${testCases.length}] ${testCase.tool} - ERROR`);
      console.log(`  ${errorMessage}`);
      console.log("");
    }
  }

  // Print summary
  console.log("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
  console.log("📊 Evaluation Summary");
  console.log(`Total: ${testCases.length}`);
  console.log(`✅ Allowed: ${passed}`);
  console.log(`❌ Denied: ${denied}`);
  console.log(`👤 Needs Human: ${needsHuman}`);
  if (errors > 0) {
    console.log(`⚠️  Errors: ${errors}`);
  }
  console.log("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

  // Write output file if requested
  if (outputPath) {
    const outputLines = results.map((r) => JSON.stringify(r)).join("\n");
    writeFileSync(outputPath, outputLines + "\n", "utf-8");
    console.log(`\n📝 Results written to: ${outputPath}`);
  }
}

runEvaluation().catch((err) => {
  console.error(`Fatal error: ${err instanceof Error ? err.message : String(err)}`);
  process.exit(1);
});
