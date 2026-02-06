import React from "react";
import type { SessionSummary } from "../api/gateway";

type Props = {
  sessions: SessionSummary[];
  activeSessionKey?: string;
  creating: boolean;
  createError?: string;
  onNewSession: () => void;
  onSelectSession: (sessionKey: string) => void;
  onToggleFavorite: (sessionKey: string) => void;
};

export function Sidebar({
  sessions,
  activeSessionKey,
  creating,
  createError,
  onNewSession,
  onSelectSession,
  onToggleFavorite,
}: Props) {
  return (
    <aside className="sidebar">
      <div className="sidebar-header">
        <div className="sidebar-title">Chats</div>
        <button
          className={`btn btn-sm new-chat-btn ${creating ? "loading" : ""}`}
          type="button"
          onClick={onNewSession}
          disabled={creating}
        >
          {creating ? "Creating…" : "+ New"}
        </button>
      </div>
      {createError ? <div className="sidebar-error">{createError}</div> : null}
      <div className="sidebar-list">
        {sessions.map((s) => {
          const isActive = s.sessionKey === activeSessionKey;
          return (
            <button
              key={s.sessionKey}
              type="button"
              className={`sidebar-item ${isActive ? "active" : ""} ${
                s.localStatus === "draft" ? "draft" : s.localStatus === "error" ? "error" : ""
              }`}
              onClick={() => onSelectSession(s.sessionKey)}
            >
              <span
                className="sidebar-item-star"
                onClick={(e) => {
                  e.stopPropagation();
                  onToggleFavorite(s.sessionKey);
                }}
              >
                {s.favorite ? "★" : "☆"}
              </span>
              <span className="sidebar-item-main">
                <span className="sidebar-item-title">
                  {s.title?.trim() || s.sessionKey.slice(0, 8)}
                </span>
                <span className="sidebar-item-meta">
                  {s.localStatus === "draft"
                    ? "Creating…"
                    : s.localStatus === "error"
                      ? "Failed to create"
                      : s.profile}
                </span>
              </span>
            </button>
          );
        })}
      </div>
    </aside>
  );
}

