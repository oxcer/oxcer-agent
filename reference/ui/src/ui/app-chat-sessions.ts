import type { OpenClawApp } from "./app.js";
import { createChatSession, listChatSessions, updateChatSession } from "./controllers/chat-sessions.js";
import { listGuardrailEvents } from "./controllers/guardrails.js";
import { listSessionActions } from "./controllers/actions.js";

type Host = {
  client: OpenClawApp["client"];
  connected: boolean;
  sessionKey: string;
  chatSessionsLoading: boolean;
  chatSessions: OpenClawApp["chatSessions"];
  chatSessionsError: string | null;
  chatSessionProfile: OpenClawApp["chatSessionProfile"];
  guardrailsTimelineLoading: boolean;
  guardrailsTimelineError: string | null;
  guardrailsTimeline: OpenClawApp["guardrailsTimeline"];
};

export async function loadChatSessions(host: Host) {
  if (!host.client || !host.connected) {
    return;
  }
  host.chatSessionsLoading = true;
  host.chatSessionsError = null;
  try {
    const sessions = await listChatSessions(host.client, { limit: 50 });
    host.chatSessions = sessions;
    const active = sessions.find((s) => s.sessionKey === host.sessionKey);
    // Always derive the dropdown from the active session metadata.
    // If we don't have it yet (e.g. switching quickly / new session), fall back to balanced.
    host.chatSessionProfile = active?.profile ?? "balanced";
  } catch (err) {
    host.chatSessionsError = String(err);
  } finally {
    host.chatSessionsLoading = false;
  }
}

export async function createNewChatSession(host: Host): Promise<string | null> {
  if (!host.client || !host.connected) {
    return null;
  }
  const session = await createChatSession(host.client, { profile: "balanced" });
  if (!session) {
    return null;
  }
  // Refresh list so sidebar shows it immediately
  await loadChatSessions(host);
  return session.sessionKey;
}

export async function setChatSessionFavorite(host: Host, sessionKey: string, favorite: boolean) {
  if (!host.client || !host.connected) {
    return;
  }
  await updateChatSession(host.client, sessionKey, { favorite });
  await loadChatSessions(host);
}

export async function setChatSessionProfile(
  host: Host,
  sessionKey: string,
  profile: "safe" | "balanced" | "experimental",
) {
  if (!host.client || !host.connected) {
    return;
  }
  const updated = await updateChatSession(host.client, sessionKey, { profile });
  if (updated?.profile) {
    host.chatSessionProfile = updated.profile;
  }
  await loadChatSessions(host);
}

export async function loadGuardrailsTimelineForActiveSession(host: Host) {
  if (!host.client || !host.connected) {
    return;
  }
  host.guardrailsTimelineLoading = true;
  host.guardrailsTimelineError = null;
  try {
    host.guardrailsTimeline = await listGuardrailEvents(host.client, {
      sessionKey: host.sessionKey,
      limit: 50,
    });
  } catch (err) {
    host.guardrailsTimelineError = String(err);
  } finally {
    host.guardrailsTimelineLoading = false;
  }
}

export async function loadSessionActionsForActiveSession(host: Host & { sessionActionsLoading: boolean; sessionActionsError: string | null; sessionActions: unknown[] }) {
  if (!host.client || !host.connected) {
    return;
  }
  host.sessionActionsLoading = true;
  host.sessionActionsError = null;
  try {
    host.sessionActions = await listSessionActions(host.client, {
      sessionKey: host.sessionKey,
      onlyExecuted: true,
      limit: 100,
    });
  } catch (err) {
    host.sessionActionsError = String(err);
  } finally {
    host.sessionActionsLoading = false;
  }
}

