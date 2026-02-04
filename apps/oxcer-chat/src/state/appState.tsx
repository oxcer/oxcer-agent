import React, { createContext, useContext, useEffect, useMemo, useState } from "react";
import { GatewayClient, type ActionRecord, type GuardrailEvent, type SessionSummary, type ChatMessage } from "../api/gateway";

export type AppState = {
  gatewayUrl: string;
  gatewayToken?: string;
  connected: boolean;
  sessions: SessionSummary[];
  sessionsLoading: boolean;
  sessionsError?: string;
  creatingSession: boolean;
  sessionCreateError?: string;
  activeSessionKey?: string;
  messagesBySession: Record<string, ChatMessage[]>;
  guardrailsEvents: GuardrailEvent[];
  guardrailsLoading: boolean;
  guardrailsError?: string;
  actions: ActionRecord[];
  actionsLoading: boolean;
  actionsError?: string;
};

type AppContextValue = {
  state: AppState;
  createNewSession: (onCreated?: () => void) => Promise<void>;
  setActiveSession: (sessionKey: string) => void;
  toggleFavorite: (sessionKey: string) => Promise<void>;
  sendMessage: (text: string) => Promise<void>;
};

const AppContext = createContext<AppContextValue | undefined>(undefined);

const DEFAULT_URL = "ws://127.0.0.1:19001";

export function AppStateProvider({ children }: { children: React.ReactNode }) {
  const [client] = useState(() => new GatewayClient({ url: DEFAULT_URL }));
  const [connected, setConnected] = useState(false);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [sessionsLoading, setSessionsLoading] = useState(false);
  const [sessionsError, setSessionsError] = useState<string | undefined>();
  const [creatingSession, setCreatingSession] = useState(false);
  const [sessionCreateError, setSessionCreateError] = useState<string | undefined>();
  const [activeSessionKey, setActiveSessionKey] = useState<string | undefined>();
  const [messagesBySession, setMessagesBySession] = useState<Record<string, ChatMessage[]>>({});
  const [guardrailsEvents, setGuardrailsEvents] = useState<GuardrailEvent[]>([]);
  const [guardrailsLoading, setGuardrailsLoading] = useState(false);
  const [guardrailsError, setGuardrailsError] = useState<string | undefined>();
  const [actions, setActions] = useState<ActionRecord[]>([]);
  const [actionsLoading, setActionsLoading] = useState(false);
  const [actionsError, setActionsError] = useState<string | undefined>();

  useEffect(() => {
    client.request("health", {}).then((res) => {
      if (res.ok) setConnected(true);
    });
  }, [client]);

  useEffect(() => {
    async function loadInitialSessions() {
      setSessionsLoading(true);
      setSessionsError(undefined);
      const res = await client.listSessions();
      if (!res.ok) {
        setSessionsError(res.error ?? "failed to load sessions");
        setSessionsLoading(false);
        return;
      }
      setSessions(res.payload ?? []);
      setSessionsLoading(false);
      const first = res.payload?.[0];
      if (!first) {
        const created = await client.createSession();
        if (created.ok && created.payload) {
          setSessions([created.payload]);
          setActiveSessionKey(created.payload.sessionKey);
          void loadSessionData(created.payload.sessionKey);
        }
      } else {
        setActiveSessionKey(first.sessionKey);
        void loadSessionData(first.sessionKey);
      }
    }
    void loadInitialSessions();
  }, [client]);

  async function loadSessionData(sessionKey: string) {
    const [historyRes, guardrailsRes, actionsRes] = await Promise.all([
      client.listChatHistory(sessionKey),
      client.listGuardrailEvents(sessionKey),
      client.listActions(sessionKey),
    ]);
    if (historyRes.ok) {
      setMessagesBySession((prev) => ({
        ...prev,
        [sessionKey]: historyRes.payload?.messages ?? [],
      }));
    }
    setGuardrailsLoading(false);
    if (guardrailsRes.ok) {
      setGuardrailsEvents(guardrailsRes.payload ?? []);
      setGuardrailsError(undefined);
    } else {
      setGuardrailsEvents([]);
      setGuardrailsError(guardrailsRes.error ?? "failed to load guardrails events");
    }
    setActionsLoading(false);
    if (actionsRes.ok) {
      setActions(actionsRes.payload ?? []);
      setActionsError(undefined);
    } else {
      setActions([]);
      setActionsError(actionsRes.error ?? "failed to load actions");
    }
  }

  const createNewSession = async (onCreated?: () => void) => {
    if (creatingSession) return;
    setCreatingSession(true);
    setSessionCreateError(undefined);

    const draftId = `draft-${Date.now()}`;
    const draft: SessionSummary = {
      sessionKey: draftId,
      title: "New chat",
      favorite: false,
      profile: "default",
      updatedAt: new Date().toISOString(),
      localStatus: "draft",
    };

    setSessions((prev) => [draft, ...prev]);
    setActiveSessionKey(draftId);
    setGuardrailsEvents([]);
    setGuardrailsError(undefined);
    setActions([]);
    setActionsError(undefined);
    setMessagesBySession((prev) => ({ ...prev, [draftId]: [] }));

    try {
      const res = await client.createSession();
      if (!res.ok || !res.payload) {
        // eslint-disable-next-line no-console
        console.error("Failed to create session", res.error);
        setSessionCreateError(res.error ?? "Failed to create chat");
        setSessions((prev) =>
          prev.map((s) =>
            s.sessionKey === draftId
              ? {
                  ...s,
                  title: "Failed to create. Click + New to retry.",
                  localStatus: "error",
                }
              : s,
          ),
        );
        return;
      }
      const session = res.payload;
      setSessions((prev) => {
        const withoutDraft = prev.filter((s) => s.sessionKey !== draftId);
        return [session, ...withoutDraft];
      });
      setActiveSessionKey(session.sessionKey);
      setGuardrailsEvents([]);
      setGuardrailsError(undefined);
      setActions([]);
      setActionsError(undefined);
      setMessagesBySession((prev) => ({ ...prev, [session.sessionKey]: [] }));
      onCreated?.();
    } finally {
      setCreatingSession(false);
    }
  };

  const setActiveSession = (sessionKey: string) => {
    setActiveSessionKey(sessionKey);
    setGuardrailsEvents([]);
    setGuardrailsError(undefined);
    setActions([]);
    setActionsError(undefined);
    void loadSessionData(sessionKey);
  };

  const toggleFavorite = async (sessionKey: string) => {
    const current = sessions.find((s) => s.sessionKey === sessionKey);
    const favorite = !current?.favorite;
    await client.updateSession(sessionKey, { favorite });
    setSessions((prev) =>
      prev.map((s) => (s.sessionKey === sessionKey ? { ...s, favorite } : s)),
    );
  };

  const sendMessage = async (text: string) => {
    if (!activeSessionKey) return;
    const res = await client.sendChatMessage(activeSessionKey, text);
    if (!res.ok) return;
    const history = await client.listChatHistory(activeSessionKey);
    if (history.ok) {
      setMessagesBySession((prev) => ({
        ...prev,
        [activeSessionKey]: history.payload?.messages ?? [],
      }));
    }
  };

  const value = useMemo<AppContextValue>(
    () => ({
      state: {
        gatewayUrl: DEFAULT_URL,
        connected,
        gatewayToken: undefined,
        sessions,
        sessionsLoading,
        sessionsError,
        creatingSession,
        sessionCreateError,
        activeSessionKey,
        messagesBySession,
        guardrailsEvents,
        guardrailsLoading,
        guardrailsError,
        actions,
        actionsLoading,
        actionsError,
      },
      createNewSession,
      setActiveSession,
      toggleFavorite,
      sendMessage,
    }),
    [
      connected,
      sessions,
      sessionsLoading,
      sessionsError,
      creatingSession,
      sessionCreateError,
      activeSessionKey,
      messagesBySession,
      guardrailsEvents,
      guardrailsLoading,
      guardrailsError,
      actions,
      actionsLoading,
      actionsError,
    ],
  );

  return <AppContext.Provider value={value}>{children}</AppContext.Provider>;
}

export function useAppState() {
  const ctx = useContext(AppContext);
  if (!ctx) {
    throw new Error("useAppState must be used within AppStateProvider");
  }
  return ctx;
}

