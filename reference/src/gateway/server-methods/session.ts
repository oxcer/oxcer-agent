/**
 * OXCER: Gateway methods for session/chat room metadata (in-memory).
 * Sprint 13: Multi-session UI scaffolding (title/favorite/profile).
 */

import type { GatewayRequestHandlers } from "./types.js";
import { ErrorCodes, errorShape } from "../protocol/index.js";
import { getActionsStore } from "../../actions/actions-store.js";
import { getSessionStore } from "../../session/session-store.js";
import type { ActionRecord, RiskLevel } from "../../actions/action-record.types.js";
import type { SessionProfile, UpdateSessionPatch } from "../../session/session.types.js";

function parseLimit(input: unknown, fallback: number): number {
  if (typeof input !== "number" || !Number.isFinite(input)) {
    return fallback;
  }
  return Math.max(1, Math.min(500, Math.floor(input)));
}

function parseProfile(input: unknown): SessionProfile | null {
  return input === "safe" || input === "balanced" || input === "experimental" ? input : null;
}

type PreExecutionHighlight = {
  actionId: string;
  riskLevel: RiskLevel;
  reason: string;
};

type PreExecutionSummary = {
  sessionKey: string;
  plannedActions: ActionRecord[];
  highlights: PreExecutionHighlight[];
};

type PostExecutionSummary = {
  sessionKey: string;
  plannedActions: ActionRecord[];
  executedActions: ActionRecord[];
  blockedActions: ActionRecord[];
  filesTouched: string[];
  sitesVisited: string[];
};

export const sessionHandlers: GatewayRequestHandlers = {
  "session.list": ({ params, respond }) => {
    const favoritesOnly = Boolean((params as { favoritesOnly?: unknown })?.favoritesOnly);
    const limit = parseLimit((params as { limit?: unknown })?.limit, 50);
    const store = getSessionStore();
    const sessions = store.listSessions({ favoritesOnly, limit });
    respond(true, { sessions });
  },

  "session.create": ({ params, respond }) => {
    const title = (params as { title?: unknown })?.title;
    const profile = parseProfile((params as { profile?: unknown })?.profile) ?? undefined;
    const store = getSessionStore();
    const session = store.createSession({
      title: typeof title === "string" ? title : undefined,
      profile,
    });
    respond(true, { session });
  },

  "session.update": ({ params, respond }) => {
    const sessionKey = (params as { sessionKey?: unknown })?.sessionKey;
    const patch = (params as { patch?: unknown })?.patch as UpdateSessionPatch | undefined;
    if (typeof sessionKey !== "string" || !sessionKey.trim()) {
      respond(
        false,
        undefined,
        errorShape(ErrorCodes.INVALID_REQUEST, "session.update requires sessionKey"),
      );
      return;
    }
    if (!patch || typeof patch !== "object") {
      respond(
        false,
        undefined,
        errorShape(ErrorCodes.INVALID_REQUEST, "session.update requires patch"),
      );
      return;
    }
    const store = getSessionStore();
    const updated = store.updateSession(sessionKey.trim(), patch);
    if (!updated) {
      respond(
        false,
        undefined,
        errorShape(ErrorCodes.INVALID_REQUEST, `session not found: ${sessionKey}`),
      );
      return;
    }
    respond(true, { session: updated });
  },

  /**
   * Sprint 14: list recorded actions for a session.
   * Params: { sessionKey: string, onlyExecuted?: boolean, limit?: number }
   */
  "session.actions.list": ({ params, respond }) => {
    const sessionKey = (params as { sessionKey?: unknown })?.sessionKey;
    if (typeof sessionKey !== "string" || !sessionKey.trim()) {
      respond(
        false,
        undefined,
        errorShape(ErrorCodes.INVALID_REQUEST, "session.actions.list requires sessionKey"),
      );
      return;
    }
    const onlyExecuted = Boolean((params as { onlyExecuted?: unknown })?.onlyExecuted);
    const limit = parseLimit((params as { limit?: unknown })?.limit, 200);
    const actions = getActionsStore().listActions({
      sessionKey: sessionKey.trim(),
      onlyExecuted,
      limit,
    });
    respond(true, { actions });
  },

  /**
   * Sprint 14: pre-execution summary derived from planned actions + risk.
   * Params: { sessionKey: string }
   */
  "session.report.preExecution": ({ params, respond }) => {
    const sessionKey = (params as { sessionKey?: unknown })?.sessionKey;
    if (typeof sessionKey !== "string" || !sessionKey.trim()) {
      respond(
        false,
        undefined,
        errorShape(ErrorCodes.INVALID_REQUEST, "session.report.preExecution requires sessionKey"),
      );
      return;
    }
    const key = sessionKey.trim();
    const plannedActions = getActionsStore()
      .listActions({ sessionKey: key, onlyExecuted: false, limit: 500 })
      .filter((a) => a.planned);

    const highlights: PreExecutionHighlight[] = [];
    for (const action of plannedActions) {
      const risk = action.riskLevel;
      const isHighlighted = risk === "medium" || risk === "high" || risk === "critical";
      if (!isHighlighted) continue;
      highlights.push({
        actionId: action.id,
        riskLevel: risk,
        reason: `Risk level: ${risk}`,
      });
    }

    const summary: PreExecutionSummary = { sessionKey: key, plannedActions, highlights };
    respond(true, summary);
  },

  /**
   * Sprint 14: post-execution report derived from recorded executed actions + metadata.
   * Params: { sessionKey: string }
   */
  "session.report.postExecution": ({ params, respond }) => {
    const sessionKey = (params as { sessionKey?: unknown })?.sessionKey;
    if (typeof sessionKey !== "string" || !sessionKey.trim()) {
      respond(
        false,
        undefined,
        errorShape(ErrorCodes.INVALID_REQUEST, "session.report.postExecution requires sessionKey"),
      );
      return;
    }
    const key = sessionKey.trim();
    const all = getActionsStore().listActions({
      sessionKey: key,
      onlyExecuted: false,
      limit: 1000,
    });
    const plannedActions = all.filter((a) => a.planned);
    const executedActions = all.filter((a) => !a.planned);
    const blockedActions = executedActions.filter(
      (a) => a.outcome === "blocked" || a.outcome === "failed",
    );
    const filesTouched = getActionsStore().deriveFilesTouched(key);
    const sitesVisited = getActionsStore().deriveSitesVisited(key);
    const report: PostExecutionSummary = {
      sessionKey: key,
      plannedActions,
      executedActions,
      blockedActions,
      filesTouched,
      sitesVisited,
    };
    respond(true, report);
  },
};
