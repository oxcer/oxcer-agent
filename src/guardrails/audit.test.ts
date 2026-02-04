import fs from "node:fs";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  getGuardrailAuditConfig,
  recordGuardrailActionDecision,
  setGuardrailAuditConfig,
} from "./audit.js";

describe("guardrail audit", () => {
  let auditDir: string;
  let auditPath: string;

  beforeEach(() => {
    auditDir = path.join(process.env.TMPDIR ?? "/tmp", `audit-test-${process.pid}-${Date.now()}`);
    auditPath = path.join(auditDir, "actions.jsonl");
    setGuardrailAuditConfig({
      enabled: true,
      filePath: auditPath,
      devConsole: false,
    });
  });

  afterEach(() => {
    setGuardrailAuditConfig(null);
    try {
      fs.rmSync(auditDir, { recursive: true });
    } catch {
      // ignore
    }
  });

  it("writes one JSONL line per recordGuardrailActionDecision when enabled", () => {
    recordGuardrailActionDecision({
      actionId: "test-action-id",
      tool: "exec",
      args: { command: "echo hi" },
      summary: "Run echo",
      timestamp: Date.now(),
      decision: "allow",
      mode: "enforce",
      failClosed: false,
      category: "action",
    });
    expect(getGuardrailAuditConfig()?.enabled).toBe(true);
    expect(fs.existsSync(auditPath)).toBe(true);
    const lines = fs.readFileSync(auditPath, "utf-8").trim().split("\n");
    expect(lines.length).toBe(1);
    const parsed = JSON.parse(lines[0]!) as Record<string, unknown>;
    expect(parsed.eventId).toBeDefined();
    expect(parsed.eventType).toBe("guardrail.action");
    expect(parsed.tool).toBe("exec");
    expect(parsed.decision).toBe("allow");
    expect(parsed.actionId).toBe("test-action-id");
    expect(typeof parsed.timestamp).toBe("string");
  });

  it("does not write when audit config is disabled", () => {
    setGuardrailAuditConfig({
      enabled: false,
      filePath: auditPath,
      devConsole: false,
    });
    recordGuardrailActionDecision({
      actionId: "no-write",
      tool: "exec",
      args: {},
      timestamp: Date.now(),
      decision: "deny",
      mode: "enforce",
      failClosed: false,
      category: "action",
    });
    expect(fs.existsSync(auditPath)).toBe(false);
  });
});
