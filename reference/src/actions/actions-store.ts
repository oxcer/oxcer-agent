import type {
  ActionFailureReason,
  ActionOutcome,
  ActionRecord,
  ListActionsOpts,
  RiskLevel,
} from "./action-record.types.js";

function nowIso(): string {
  return new Date().toISOString();
}

function generateId(prefix: string): string {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 11)}`;
}

function riskFromTool(tool: string): RiskLevel {
  const t = tool.toLowerCase();
  if (t.includes("exec") || t.includes("bash") || t.includes("terminal")) return "critical";
  if (t.includes("write") || t.includes("delete") || t.includes("remove") || t.includes("patch"))
    return "high";
  if (t.includes("browser") || t.includes("web") || t.includes("fetch") || t.includes("http"))
    return "medium";
  return "low";
}

function summarizeMeta(
  tool: string,
  args: Record<string, unknown> | undefined,
): { filePath?: string; url?: string } {
  const t = tool.toLowerCase();
  const a = args ?? {};
  const filePath =
    typeof (a as { path?: unknown }).path === "string"
      ? (a as { path: string }).path.trim() || undefined
      : undefined;
  const url =
    typeof (a as { url?: unknown }).url === "string"
      ? (a as { url: string }).url.trim() || undefined
      : undefined;
  if (
    filePath &&
    (t.includes("file") || t.includes("fs") || t.includes("write") || t.includes("read"))
  ) {
    return { filePath };
  }
  if (
    url &&
    (t.includes("browser") || t.includes("web") || t.includes("fetch") || t.includes("http"))
  ) {
    return { url };
  }
  // Allow capturing path/url even if tool name doesn't match perfectly.
  return { filePath, url };
}

export type RecordPlannedParams = {
  sessionKey: string;
  tool: string;
  summary: string;
  riskLevel?: RiskLevel;
  meta?: ActionRecord["meta"];
};

export type RecordExecutedParams = RecordPlannedParams & {
  outcome: ActionOutcome;
  failureReason?: ActionFailureReason;
};

class ActionsStore {
  private actions: ActionRecord[] = [];
  private maxActions = 3000;

  recordPlannedAction(params: RecordPlannedParams): ActionRecord {
    const record: ActionRecord = {
      id: generateId("plan"),
      sessionKey: params.sessionKey,
      timestamp: nowIso(),
      tool: params.tool,
      summary: params.summary,
      planned: true,
      riskLevel: params.riskLevel ?? riskFromTool(params.tool),
      meta: params.meta,
    };
    this.push(record);
    return record;
  }

  recordExecutedAction(params: RecordExecutedParams): ActionRecord {
    const record: ActionRecord = {
      id: generateId("act"),
      sessionKey: params.sessionKey,
      timestamp: nowIso(),
      tool: params.tool,
      summary: params.summary,
      planned: false,
      outcome: params.outcome,
      failureReason: params.failureReason,
      riskLevel: params.riskLevel ?? riskFromTool(params.tool),
      meta: params.meta,
    };
    this.push(record);
    return record;
  }

  listActions(opts?: ListActionsOpts): ActionRecord[] {
    const sessionKey = opts?.sessionKey?.trim();
    const onlyExecuted = opts?.onlyExecuted === true;
    const limit =
      typeof opts?.limit === "number" && Number.isFinite(opts.limit) ? Math.floor(opts.limit) : 200;
    let list = this.actions;
    if (sessionKey) {
      list = list.filter((a) => a.sessionKey === sessionKey);
    }
    if (onlyExecuted) {
      list = list.filter((a) => a.planned === false);
    }
    // Newest first
    list = [...list].sort((a, b) => b.timestamp.localeCompare(a.timestamp));
    return list.slice(0, Math.max(1, Math.min(1000, limit)));
  }

  deriveFilesTouched(sessionKey: string): string[] {
    const files = new Set<string>();
    for (const action of this.actions) {
      if (action.sessionKey !== sessionKey || action.planned) continue;
      const filePath = action.meta?.filePath;
      if (filePath) files.add(filePath);
    }
    return [...files].slice(0, 200);
  }

  deriveSitesVisited(sessionKey: string): string[] {
    const sites = new Set<string>();
    for (const action of this.actions) {
      if (action.sessionKey !== sessionKey || action.planned) continue;
      const url = action.meta?.url;
      if (!url) continue;
      try {
        const parsed = new URL(url);
        sites.add(parsed.host ? parsed.host : url);
      } catch {
        sites.add(url);
      }
    }
    return [...sites].slice(0, 200);
  }

  private push(action: ActionRecord) {
    this.actions.push(action);
    if (this.actions.length > this.maxActions) {
      this.actions.splice(0, this.actions.length - this.maxActions);
    }
  }
}

let store: ActionsStore | null = null;

export function getActionsStore(): ActionsStore {
  if (!store) {
    store = new ActionsStore();
  }
  return store;
}

export function resetActionsStore(): void {
  store = null;
}

export function deriveActionMetaFromToolArgs(
  tool: string,
  args: Record<string, unknown> | undefined,
) {
  return summarizeMeta(tool, args);
}
