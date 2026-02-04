/**
 * OXCER: Pattern-based failure tagging for guardrails records.
 * Sprint 10: lightweight, modular tagger to group similar failure modes.
 *
 * This is intentionally simple and conservative; it is meant for clustering and
 * prioritization, not as a security classifier.
 */

export type GuardrailFailureCategory =
  | "destructive_command"
  | "sensitive_file_read"
  | "credential_exfiltration"
  | "prompt_injection"
  | "system_prompt_leak"
  | "unsafe_write_path"
  | "unknown";

export type TaggerInput = {
  railType: "input" | "action" | "result";
  inputSummary: string;
  tool?: string;
  args?: Record<string, unknown>;
  reason?: string;
};

function asString(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function concatSignals(input: TaggerInput): string {
  const tool = input.tool ?? "";
  const summary = input.inputSummary ?? "";
  const reason = input.reason ?? "";
  const cmd = asString(input.args?.command);
  const path = asString(input.args?.path);
  const url = asString(input.args?.url);
  return [tool, summary, reason, cmd, path, url].join("\n").toLowerCase();
}

export function tagFailureCategory(input: TaggerInput): GuardrailFailureCategory | undefined {
  const text = concatSignals(input);

  // Destructive commands (very high-risk)
  if (
    /\brm\s+-rf\b/.test(text) ||
    /\brm\s+-r\b/.test(text) ||
    /\bmkfs\b/.test(text) ||
    /\bdd\s+if=/.test(text)
  ) {
    return "destructive_command";
  }

  // Sensitive file reads
  if (
    /\/etc\/passwd\b/.test(text) ||
    /\/etc\/shadow\b/.test(text) ||
    /id_rsa\b/.test(text) ||
    /authorized_keys\b/.test(text) ||
    /\b\.aws\/credentials\b/.test(text) ||
    /\b\.env\b/.test(text)
  ) {
    return "sensitive_file_read";
  }

  // Exfiltration-ish signals
  if (
    /\bcurl\b/.test(text) ||
    /\bwget\b/.test(text) ||
    /\bnc\b/.test(text) ||
    /\bnetcat\b/.test(text) ||
    /https?:\/\/.+/.test(text)
  ) {
    // If it also mentions secrets/keys, call it out as credential exfil.
    if (
      /\bapi[_-]?key\b/.test(text) ||
      /\btoken\b/.test(text) ||
      /\bsecret\b/.test(text) ||
      /\bpassword\b/.test(text)
    ) {
      return "credential_exfiltration";
    }
  }

  // Prompt injection signals
  if (
    /ignore previous instructions/.test(text) ||
    /override system/.test(text) ||
    /you are now/.test(text) ||
    /developer message/.test(text)
  ) {
    return "prompt_injection";
  }

  // System prompt leak hints
  if (/system prompt/.test(text) || /reveal.*system/.test(text) || /show.*system/.test(text)) {
    return "system_prompt_leak";
  }

  // Unsafe write paths (overly broad; just a starting point)
  if (
    /\bwrite\b/.test(text) &&
    (/\/etc\//.test(text) || /\/bin\//.test(text) || /\/usr\/bin\//.test(text))
  ) {
    return "unsafe_write_path";
  }

  return undefined;
}
