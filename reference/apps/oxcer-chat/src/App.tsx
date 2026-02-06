import React, { useRef } from "react";
import { AppStateProvider, useAppState } from "./state/appState";
import { Sidebar } from "./components/Sidebar";
import { ChatPanel } from "./components/ChatPanel";
import "./styles.css";

function AppInner() {
  const { state, createNewSession, setActiveSession, toggleFavorite, sendMessage } =
    useAppState();
  const inputRef = useRef<HTMLTextAreaElement | null>(null);

  const activeSession = state.sessions.find((s) => s.sessionKey === state.activeSessionKey);
  const messages =
    (state.activeSessionKey && state.messagesBySession[state.activeSessionKey]) ?? [];

  return (
    <div className="app-shell">
      <Sidebar
        sessions={state.sessions}
        activeSessionKey={state.activeSessionKey}
        creating={state.creatingSession}
        createError={state.sessionCreateError}
        onNewSession={() => void createNewSession(() => inputRef.current?.focus())}
        onSelectSession={(key) => setActiveSession(key)}
        onToggleFavorite={(key) => void toggleFavorite(key)}
      />
      <main className="app-main">
        {!activeSession ? (
          <div className="empty-state">
            <h1>Oxcer Chat</h1>
            <p>Create a new chat on the left to get started.</p>
          </div>
        ) : (
          <ChatPanel
            sessionTitle={activeSession.title ?? activeSession.sessionKey.slice(0, 8)}
            messages={messages}
            guardrailsEvents={state.guardrailsEvents}
            actions={state.actions}
            onSend={(text) => void sendMessage(text)}
            inputRef={inputRef}
          />
        )}
      </main>
    </div>
  );
}

export function App() {
  return (
    <AppStateProvider>
      <AppInner />
    </AppStateProvider>
  );
}

