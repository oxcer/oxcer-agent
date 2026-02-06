import React, { useMemo, useState } from "react";
import type { ActionRecord, GuardrailEvent, ChatMessage } from "../api/gateway";

type Props = {
  sessionTitle: string;
  messages: ChatMessage[];
  guardrailsEvents: GuardrailEvent[];
  actions: ActionRecord[];
  onSend: (text: string) => void;
  inputRef?: React.RefObject<HTMLTextAreaElement>;
};

export function ChatPanel({
  sessionTitle,
  messages,
  guardrailsEvents,
  actions,
  onSend,
  inputRef,
}: Props) {
  const [draft, setDraft] = useState("");
  const [tab, setTab] = useState<"chat" | "guardrails" | "actions">("chat");

  const sortedMessages = useMemo(
    () => [...messages].sort((a, b) => (a.timestamp ?? 0) - (b.timestamp ?? 0)),
    [messages],
  );

  return (
    <section className="chat-panel">
      <header className="chat-header">
        <div className="chat-title">{sessionTitle}</div>
      </header>

      <div className="chat-tabs">
        <button
          type="button"
          className={tab === "chat" ? "active" : ""}
          onClick={() => setTab("chat")}
        >
          Chat
        </button>
        <button
          type="button"
          className={tab === "guardrails" ? "active" : ""}
          onClick={() => setTab("guardrails")}
        >
          Guardrails
        </button>
        <button
          type="button"
          className={tab === "actions" ? "active" : ""}
          onClick={() => setTab("actions")}
        >
          Actions
        </button>
      </div>

      <div className="chat-body">
        {tab === "chat" && (
          <div className="chat-messages">
            {sortedMessages.length === 0 ? (
              <div className="muted">No messages yet. Start the conversation below.</div>
            ) : (
              sortedMessages.map((m, idx) => (
                <div key={idx} className={`chat-message chat-message-${m.role.toLowerCase()}`}>
                  <div className="chat-message-role">{m.role}</div>
                  <div className="chat-message-content">
                    {typeof m.content === "string"
                      ? m.content
                      : JSON.stringify(m.content, null, 2)}
                  </div>
                </div>
              ))
            )}
          </div>
        )}

        {tab === "guardrails" && (
          <div className="chat-guardrails">
            {guardrailsEvents.length === 0 ? (
              <div className="muted">No guardrails events for this session yet.</div>
            ) : (
              guardrailsEvents.map((ev) => (
                <div key={ev.id} className="guardrail-row">
                  <span className="mono">
                    {new Date(ev.timestamp).toLocaleTimeString()}
                  </span>
                  <span className="pill">{ev.decision}</span>
                  <span className="mono">{ev.tool}</span>
                  <span>{ev.summary}</span>
                </div>
              ))
            )}
          </div>
        )}

        {tab === "actions" && (
          <div className="chat-actions">
            {actions.length === 0 ? (
              <div className="muted">No actions for this session yet.</div>
            ) : (
              actions.map((a) => (
                <div key={a.id} className="actions-row">
                  <span className="mono">
                    {new Date(a.timestamp).toLocaleTimeString()}
                  </span>
                  <span className="mono">{a.tool}</span>
                  <span>{a.summary}</span>
                  <span className="pill">{a.riskLevel}</span>
                </div>
              ))
            )}
          </div>
        )}
      </div>

      <footer className="chat-footer">
        <textarea
          ref={inputRef}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          placeholder="Message the assistant..."
        />
        <button
          type="button"
          className="btn primary"
          onClick={() => {
            const text = draft.trim();
            if (!text) return;
            onSend(text);
            setDraft("");
          }}
        >
          Send
        </button>
      </footer>
    </section>
  );
}

