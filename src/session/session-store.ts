import { randomUUID } from "node:crypto";
import type {
  CreateSessionParams,
  SessionInfo,
  SessionProfile,
  UpdateSessionPatch,
} from "./session.types.js";

type ListSessionsOpts = {
  favoritesOnly?: boolean;
  limit?: number;
};

function nowIso(): string {
  return new Date().toISOString();
}

function normalizeProfile(input: unknown): SessionProfile {
  return input === "safe" || input === "balanced" || input === "experimental" ? input : "balanced";
}

function generateSessionKey(): string {
  return `chat:${randomUUID()}`;
}

class SessionStore {
  private sessions = new Map<string, SessionInfo>();

  ensureSession(sessionKey: string, opts?: { profile?: SessionProfile }): SessionInfo {
    const key = sessionKey.trim();
    if (!key) {
      throw new Error("sessionKey is required");
    }
    const existing = this.sessions.get(key);
    if (existing) {
      return existing;
    }
    const createdAt = nowIso();
    const info: SessionInfo = {
      sessionKey: key,
      createdAt,
      updatedAt: createdAt,
      profile: opts?.profile ?? "balanced",
    };
    this.sessions.set(key, info);
    return info;
  }

  createSession(params?: CreateSessionParams): SessionInfo {
    const requested = params?.sessionKey?.trim();
    const sessionKey = requested || generateSessionKey();
    const createdAt = nowIso();
    const info: SessionInfo = {
      sessionKey,
      title: params?.title?.trim() || undefined,
      favorite: params?.favorite ?? false,
      profile: normalizeProfile(params?.profile),
      createdAt,
      updatedAt: createdAt,
    };
    this.sessions.set(sessionKey, info);
    return info;
  }

  getSession(sessionKey: string): SessionInfo | null {
    return this.sessions.get(sessionKey) ?? null;
  }

  listSessions(opts?: ListSessionsOpts): SessionInfo[] {
    const favoritesOnly = opts?.favoritesOnly === true;
    const limit =
      typeof opts?.limit === "number" && Number.isFinite(opts.limit) ? opts.limit : undefined;
    let sessions = Array.from(this.sessions.values());
    if (favoritesOnly) {
      sessions = sessions.filter((s) => s.favorite === true);
    }
    sessions.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
    if (limit != null && limit > 0) {
      sessions = sessions.slice(0, limit);
    }
    return sessions;
  }

  updateSession(sessionKey: string, patch: UpdateSessionPatch): SessionInfo | null {
    const existing = this.sessions.get(sessionKey);
    if (!existing) {
      return null;
    }
    const next: SessionInfo = {
      ...existing,
      title: typeof patch.title === "string" ? patch.title.trim() || undefined : existing.title,
      favorite: typeof patch.favorite === "boolean" ? patch.favorite : existing.favorite,
      profile: patch.profile ? normalizeProfile(patch.profile) : existing.profile,
      updatedAt: nowIso(),
    };
    this.sessions.set(sessionKey, next);
    return next;
  }

  touchSession(sessionKey: string): SessionInfo {
    const existing = this.ensureSession(sessionKey);
    const next: SessionInfo = { ...existing, updatedAt: nowIso() };
    this.sessions.set(sessionKey, next);
    return next;
  }
}

let store: SessionStore | null = null;

export function getSessionStore(): SessionStore {
  if (!store) {
    store = new SessionStore();
  }
  return store;
}

export function resetSessionStore(): void {
  store = null;
}
