/**
 * OXCER: Chat session (room) controller for UI.
 * Sprint 13: In-memory session registry via gateway `session.*` methods.
 */

import type { Gateway } from "../gateway.js";

export type SessionProfile = "safe" | "balanced" | "experimental";

export type SessionInfo = {
  sessionKey: string;
  title?: string;
  createdAt: string;
  updatedAt: string;
  favorite?: boolean;
  profile: SessionProfile;
};

export async function listChatSessions(
  gateway: Gateway,
  opts?: { favoritesOnly?: boolean; limit?: number },
): Promise<SessionInfo[]> {
  const result = await gateway.request("session.list", opts ?? {});
  if (!result.ok || !result.payload) {
    return [];
  }
  const payload = result.payload as { sessions?: SessionInfo[] };
  return payload.sessions ?? [];
}

export async function createChatSession(
  gateway: Gateway,
  opts?: { title?: string; profile?: SessionProfile },
): Promise<SessionInfo | null> {
  const result = await gateway.request("session.create", opts ?? {});
  if (!result.ok || !result.payload) {
    return null;
  }
  const payload = result.payload as { session?: SessionInfo };
  return payload.session ?? null;
}

export async function updateChatSession(
  gateway: Gateway,
  sessionKey: string,
  patch: Partial<Pick<SessionInfo, "title" | "favorite" | "profile">>,
): Promise<SessionInfo | null> {
  const result = await gateway.request("session.update", { sessionKey, patch });
  if (!result.ok || !result.payload) {
    return null;
  }
  const payload = result.payload as { session?: SessionInfo };
  return payload.session ?? null;
}

