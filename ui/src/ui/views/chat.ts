import { html, nothing } from "lit";
import { ref } from "lit/directives/ref.js";
import { repeat } from "lit/directives/repeat.js";
import type { SessionsListResult } from "../types";
import type { ChatItem, MessageGroup } from "../types/chat-types";
import type { ChatAttachment, ChatQueueItem } from "../ui-types";
import {
  renderMessageGroup,
  renderReadingIndicatorGroup,
  renderStreamingGroup,
} from "../chat/grouped-render";
import { normalizeMessage, normalizeRoleForGrouping } from "../chat/message-normalizer";
import { icons } from "../icons";
import { renderMarkdownSidebar } from "./markdown-sidebar";
import "../components/resizable-divider";

export type CompactionIndicatorStatus = {
  active: boolean;
  startedAt: number | null;
  completedAt: number | null;
};

export type ChatProps = {
  sessionKey: string;
  onSessionKeyChange: (next: string) => void;
  thinkingLevel: string | null;
  showThinking: boolean;
  loading: boolean;
  sending: boolean;
  canAbort?: boolean;
  compactionStatus?: CompactionIndicatorStatus | null;
  messages: unknown[];
  toolMessages: unknown[];
  stream: string | null;
  streamStartedAt: number | null;
  assistantAvatarUrl?: string | null;
  draft: string;
  queue: ChatQueueItem[];
  connected: boolean;
  canSend: boolean;
  disabledReason: string | null;
  error: string | null;
  sessions: SessionsListResult | null;
  // Focus mode
  focusMode: boolean;
  // Sidebar state
  sidebarOpen?: boolean;
  sidebarContent?: string | null;
  sidebarError?: string | null;
  splitRatio?: number;
  assistantName: string;
  assistantAvatar: string | null;
  // Image attachments
  attachments?: ChatAttachment[];
  onAttachmentsChange?: (attachments: ChatAttachment[]) => void;
  // Scroll control
  showNewMessages?: boolean;
  onScrollToBottom?: () => void;
  // Event handlers
  onRefresh: () => void;
  onToggleFocusMode: () => void;
  onDraftChange: (next: string) => void;
  onSend: () => void;
  onAbort?: () => void;
  onQueueRemove: (id: string) => void;
  onNewSession: () => void;
  // Sprint 13: multi-session sidebar + per-session profile selector + per-session guardrails timeline
  chatSessionsLoading?: boolean;
  chatSessions?: Array<{
    sessionKey: string;
    title?: string;
    updatedAt: string;
    favorite?: boolean;
    profile: "safe" | "balanced" | "experimental";
  }>;
  chatSessionsError?: string | null;
  activeProfile?: "safe" | "balanced" | "experimental";
  onCreateChatSession?: () => void | Promise<void>;
  onToggleFavorite?: (sessionKey: string, favorite: boolean) => void | Promise<void>;
  onProfileChange?: (profile: "safe" | "balanced" | "experimental") => void | Promise<void>;
  guardrailsTimelineLoading?: boolean;
  guardrailsTimelineError?: string | null;
  guardrailsTimeline?: Array<{
    id: string;
    timestamp: number;
    type: "action" | "result";
    decision: "allow" | "deny" | "needs_human";
    summary: string;
    tool: string;
    status: "pending_review" | "resolved" | "auto_resolved";
    humanDecision?: "approve" | "reject";
  }>;
  // Sprint 14: per-session actions + reports
  sessionActionsLoading?: boolean;
  sessionActionsError?: string | null;
  sessionActions?: Array<{
    id: string;
    timestamp: string;
    tool: string;
    summary: string;
    outcome?: "success" | "failed" | "blocked" | "skipped";
    failureReason?: string;
    riskLevel: "low" | "medium" | "high" | "critical";
  }>;
  onOpenPreExecutionReport?: () => void | Promise<void>;
  onClosePreExecutionReport?: () => void;
  preExecutionReportOpen?: boolean;
  preExecutionReportLoading?: boolean;
  preExecutionReportError?: string | null;
  preExecutionReport?: {
    sessionKey: string;
    plannedActions: Array<{ id: string; summary: string; tool: string; riskLevel: string }>;
    highlights: Array<{ actionId: string; riskLevel: string; reason: string }>;
  } | null;
  onOpenPostExecutionReport?: () => void | Promise<void>;
  onClosePostExecutionReport?: () => void;
  postExecutionReportOpen?: boolean;
  postExecutionReportLoading?: boolean;
  postExecutionReportError?: string | null;
  postExecutionReport?: {
    sessionKey: string;
    plannedActions: Array<{ id: string; summary: string; tool: string; riskLevel: string }>;
    executedActions: Array<{ id: string; summary: string; tool: string; outcome?: string; riskLevel: string }>;
    blockedActions: Array<{ id: string; summary: string; tool: string; outcome?: string; riskLevel: string }>;
    filesTouched: string[];
    sitesVisited: string[];
  } | null;
  onOpenSidebar?: (content: string) => void;
  onCloseSidebar?: () => void;
  onSplitRatioChange?: (ratio: number) => void;
  onChatScroll?: (event: Event) => void;
};

const COMPACTION_TOAST_DURATION_MS = 5000;

function adjustTextareaHeight(el: HTMLTextAreaElement) {
  el.style.height = "auto";
  el.style.height = `${el.scrollHeight}px`;
}

function renderCompactionIndicator(status: CompactionIndicatorStatus | null | undefined) {
  if (!status) {
    return nothing;
  }

  // Show "compacting..." while active
  if (status.active) {
    return html`
      <div class="callout info compaction-indicator compaction-indicator--active">
        ${icons.loader} Compacting context...
      </div>
    `;
  }

  // Show "compaction complete" briefly after completion
  if (status.completedAt) {
    const elapsed = Date.now() - status.completedAt;
    if (elapsed < COMPACTION_TOAST_DURATION_MS) {
      return html`
        <div class="callout success compaction-indicator compaction-indicator--complete">
          ${icons.check} Context compacted
        </div>
      `;
    }
  }

  return nothing;
}

function generateAttachmentId(): string {
  return `att-${Date.now()}-${Math.random().toString(36).slice(2, 9)}`;
}

function handlePaste(e: ClipboardEvent, props: ChatProps) {
  const items = e.clipboardData?.items;
  if (!items || !props.onAttachmentsChange) {
    return;
  }

  const imageItems: DataTransferItem[] = [];
  for (let i = 0; i < items.length; i++) {
    const item = items[i];
    if (item.type.startsWith("image/")) {
      imageItems.push(item);
    }
  }

  if (imageItems.length === 0) {
    return;
  }

  e.preventDefault();

  for (const item of imageItems) {
    const file = item.getAsFile();
    if (!file) {
      continue;
    }

    const reader = new FileReader();
    reader.addEventListener("load", () => {
      const dataUrl = reader.result as string;
      const newAttachment: ChatAttachment = {
        id: generateAttachmentId(),
        dataUrl,
        mimeType: file.type,
      };
      const current = props.attachments ?? [];
      props.onAttachmentsChange?.([...current, newAttachment]);
    });
    reader.readAsDataURL(file);
  }
}

function renderAttachmentPreview(props: ChatProps) {
  const attachments = props.attachments ?? [];
  if (attachments.length === 0) {
    return nothing;
  }

  return html`
    <div class="chat-attachments">
      ${attachments.map(
        (att) => html`
          <div class="chat-attachment">
            <img
              src=${att.dataUrl}
              alt="Attachment preview"
              class="chat-attachment__img"
            />
            <button
              class="chat-attachment__remove"
              type="button"
              aria-label="Remove attachment"
              @click=${() => {
                const next = (props.attachments ?? []).filter((a) => a.id !== att.id);
                props.onAttachmentsChange?.(next);
              }}
            >
              ${icons.x}
            </button>
          </div>
        `,
      )}
    </div>
  `;
}

export function renderChat(props: ChatProps) {
  const canCompose = props.connected;
  const isBusy = props.sending || props.stream !== null;
  const canAbort = Boolean(props.canAbort && props.onAbort);
  const activeSession = props.sessions?.sessions?.find((row) => row.key === props.sessionKey);
  const reasoningLevel = activeSession?.reasoningLevel ?? "off";
  const showReasoning = props.showThinking && reasoningLevel !== "off";
  const assistantIdentity = {
    name: props.assistantName,
    avatar: props.assistantAvatar ?? props.assistantAvatarUrl ?? null,
  };

  const hasAttachments = (props.attachments?.length ?? 0) > 0;
  const composePlaceholder = props.connected
    ? hasAttachments
      ? "Add a message or paste more images..."
      : "Message (↩ to send, Shift+↩ for line breaks, paste images)"
    : "Connect to the gateway to start chatting…";

  const splitRatio = props.splitRatio ?? 0.6;
  const sidebarOpen = Boolean(props.sidebarOpen && props.onCloseSidebar);

  const sessionList = props.chatSessions ?? [];
  const activeProfile = props.activeProfile ?? "balanced";
  const timeline = props.guardrailsTimeline ?? [];
  const timelineLoading = Boolean(props.guardrailsTimelineLoading);
  const timelineError = props.guardrailsTimelineError ?? null;
  const actions = props.sessionActions ?? [];
  const actionsLoading = Boolean(props.sessionActionsLoading);
  const actionsError = props.sessionActionsError ?? null;
  const preOpen = Boolean(props.preExecutionReportOpen);
  const postOpen = Boolean(props.postExecutionReportOpen);

  const renderRiskBadge = (risk: string) => html`<span class="badge badge--risk badge--risk-${risk}">${risk}</span>`;
  const renderOutcomeBadge = (outcome?: string, reason?: string) => {
    if (!outcome) return nothing;
    const label = reason ? `${outcome} (${reason})` : outcome;
    const cls = outcome === "success" ? "badge--ok" : outcome === "blocked" ? "badge--warn" : "badge--danger";
    return html`<span class="badge ${cls}">${label}</span>`;
  };

  const renderPreExecutionModal = () => {
    if (!preOpen) return nothing;
    const report = props.preExecutionReport;
    return html`
      <div class="modal-backdrop" @click=${() => props.onClosePreExecutionReport?.()}>
        <div class="modal" @click=${(e: Event) => e.stopPropagation()}>
          <div class="modal__header">
            <div class="modal__title">Pre-execution summary</div>
            <button class="btn btn--sm" type="button" @click=${() => props.onClosePreExecutionReport?.()}>Close</button>
          </div>
          ${props.preExecutionReportError
            ? html`<div class="callout danger">${props.preExecutionReportError}</div>`
            : nothing}
          ${props.preExecutionReportLoading
            ? html`<div class="muted">Loading…</div>`
            : report
              ? html`
                  <div class="modal__section">
                    <div class="modal__section-title">Planned actions</div>
                    <div class="actions-list">
                      ${report.plannedActions.map(
                        (a) => html`
                          <div class="actions-row">
                            ${renderRiskBadge(String(a.riskLevel))}
                            <span class="mono">${a.tool}</span>
                            <span class="actions-row__summary">${a.summary}</span>
                          </div>
                        `,
                      )}
                    </div>
                  </div>
                  <div class="modal__section">
                    <div class="modal__section-title">Highlights</div>
                    ${report.highlights.length === 0
                      ? html`<div class="muted">No highlights.</div>`
                      : html`
                          <ul>
                            ${report.highlights.map(
                              (h) => html`<li>${renderRiskBadge(String(h.riskLevel))} ${h.reason}</li>`,
                            )}
                          </ul>
                        `}
                  </div>
                `
              : html`<div class="muted">No report available.</div>`}
        </div>
      </div>
    `;
  };

  const renderPostExecutionModal = () => {
    if (!postOpen) return nothing;
    const report = props.postExecutionReport;
    return html`
      <div class="modal-backdrop" @click=${() => props.onClosePostExecutionReport?.()}>
        <div class="modal" @click=${(e: Event) => e.stopPropagation()}>
          <div class="modal__header">
            <div class="modal__title">Post-execution report</div>
            <button class="btn btn--sm" type="button" @click=${() => props.onClosePostExecutionReport?.()}>Close</button>
          </div>
          ${props.postExecutionReportError
            ? html`<div class="callout danger">${props.postExecutionReportError}</div>`
            : nothing}
          ${props.postExecutionReportLoading
            ? html`<div class="muted">Loading…</div>`
            : report
              ? html`
                  <div class="modal__section">
                    <div class="modal__section-title">Summary</div>
                    <div class="pill">
                      ${report.plannedActions.length} planned, ${report.executedActions.length} executed, ${report.blockedActions.length} blocked/failed
                    </div>
                  </div>
                  <div class="modal__section">
                    <div class="modal__section-title">Files touched</div>
                    ${report.filesTouched.length
                      ? html`<ul>${report.filesTouched.map((p) => html`<li class="mono">${p}</li>`)}</ul>`
                      : html`<div class="muted">None</div>`}
                  </div>
                  <div class="modal__section">
                    <div class="modal__section-title">Sites visited</div>
                    ${report.sitesVisited.length
                      ? html`<ul>${report.sitesVisited.map((s) => html`<li class="mono">${s}</li>`)}</ul>`
                      : html`<div class="muted">None</div>`}
                  </div>
                `
              : html`<div class="muted">No report available.</div>`}
        </div>
      </div>
    `;
  };

  const renderSessionSidebar = () => html`
    <div class="chat-sessions">
      <div class="chat-sessions__header">
        <div class="chat-sessions__title">Chats</div>
        <button
          class="btn btn--sm"
          type="button"
          ?disabled=${!props.connected || !props.onCreateChatSession}
          @click=${() => props.onCreateChatSession?.()}
          title="New chat"
        >
          + New
        </button>
      </div>
      ${props.chatSessionsError
        ? html`<div class="callout danger">Sessions error: ${props.chatSessionsError}</div>`
        : nothing}
      ${props.chatSessionsLoading
        ? html`<div class="muted">Loading sessions…</div>`
        : html`
            <div class="chat-sessions__list">
              ${repeat(
                sessionList,
                (s) => s.sessionKey,
                (s) => html`
                  <button
                    class="chat-sessions__item ${s.sessionKey === props.sessionKey ? "active" : ""}"
                    type="button"
                    @click=${() => props.onSessionKeyChange(s.sessionKey)}
                    title=${s.sessionKey}
                  >
                    <span
                      class="chat-sessions__item-star"
                      @click=${(e: Event) => {
                        e.preventDefault();
                        e.stopPropagation();
                        props.onToggleFavorite?.(s.sessionKey, !Boolean(s.favorite));
                      }}
                      title=${s.favorite ? "Unfavorite" : "Favorite"}
                      aria-label=${s.favorite ? "Unfavorite" : "Favorite"}
                    >
                      ${s.favorite ? "★" : "☆"}
                    </span>
                    <span class="chat-sessions__item-main">
                      <span class="chat-sessions__item-title">
                        ${s.title?.trim() || s.sessionKey}
                      </span>
                      <span class="chat-sessions__item-meta">${s.profile}</span>
                    </span>
                  </button>
                `,
              )}
            </div>
          `}
      <div class="chat-sessions__divider"></div>
      <div class="chat-sessions__section">
        <div class="chat-sessions__section-title">Profile</div>
        <select
          class="chat-sessions__profile"
          .value=${activeProfile}
          ?disabled=${!props.connected || !props.onProfileChange}
          @change=${(e: Event) => {
            const next = (e.target as HTMLSelectElement).value as
              | "safe"
              | "balanced"
              | "experimental";
            props.onProfileChange?.(next);
          }}
        >
          <option value="safe">safe</option>
          <option value="balanced">balanced</option>
          <option value="experimental">experimental</option>
        </select>
      </div>
      <div class="chat-sessions__divider"></div>
      <div class="chat-sessions__section">
        <div class="chat-sessions__section-title">Guardrails (this chat)</div>
        ${timelineError ? html`<div class="callout danger">${timelineError}</div>` : nothing}
        ${timelineLoading
          ? html`<div class="muted">Loading guardrails…</div>`
          : html`
              <div class="chat-sessions__timeline">
                ${timeline.length === 0
                  ? html`<div class="muted">No guardrails events yet.</div>`
                  : repeat(
                      timeline,
                      (ev) => ev.id,
                      (ev) => html`
                        <div class="chat-sessions__timeline-item">
                          <div class="chat-sessions__timeline-title">
                            <span class="mono">${new Date(ev.timestamp).toLocaleTimeString()}</span>
                            <span class="pill">${ev.decision}</span>
                            <span class="pill">${ev.type}</span>
                          </div>
                          <div class="chat-sessions__timeline-body">
                            <div>${ev.summary}</div>
                            <div class="muted mono">${ev.tool}</div>
                          </div>
                        </div>
                      `,
                    )}
              </div>
            `}
      </div>
      <div class="chat-sessions__divider"></div>
      <div class="chat-sessions__section">
        <div class="chat-sessions__section-title">Actions (this chat)</div>
        <div class="chat-sessions__actions-buttons">
          <button
            class="btn btn--sm"
            type="button"
            ?disabled=${!props.connected || !props.onOpenPreExecutionReport}
            @click=${() => props.onOpenPreExecutionReport?.()}
          >
            Preview
          </button>
          <button
            class="btn btn--sm"
            type="button"
            ?disabled=${!props.connected || !props.onOpenPostExecutionReport}
            @click=${() => props.onOpenPostExecutionReport?.()}
          >
            Report
          </button>
        </div>
        ${actionsError ? html`<div class="callout danger">${actionsError}</div>` : nothing}
        ${actionsLoading
          ? html`<div class="muted">Loading actions…</div>`
          : html`
              <div class="actions-list">
                ${actions.length === 0
                  ? html`<div class="muted">No actions for this session yet.</div>`
                  : actions.map(
                      (a) => html`
                        <div class="actions-row">
                          <span class="mono">${new Date(a.timestamp).toLocaleTimeString()}</span>
                          ${renderRiskBadge(a.riskLevel)}
                          <span class="mono actions-row__tool">${a.tool}</span>
                          <span class="actions-row__summary">${a.summary}</span>
                          ${renderOutcomeBadge(a.outcome, a.failureReason)}
                        </div>
                      `,
                    )}
              </div>
            `}
      </div>
    </div>
  `;
  const thread = html`
    <div
      class="chat-thread"
      role="log"
      aria-live="polite"
      @scroll=${props.onChatScroll}
    >
      ${
        props.loading
          ? html`
              <div class="muted">Loading chat…</div>
            `
          : nothing
      }
      ${repeat(
        buildChatItems(props),
        (item) => item.key,
        (item) => {
          if (item.kind === "reading-indicator") {
            return renderReadingIndicatorGroup(assistantIdentity);
          }

          if (item.kind === "stream") {
            return renderStreamingGroup(
              item.text,
              item.startedAt,
              props.onOpenSidebar,
              assistantIdentity,
            );
          }

          if (item.kind === "group") {
            return renderMessageGroup(item, {
              onOpenSidebar: props.onOpenSidebar,
              showReasoning,
              assistantName: props.assistantName,
              assistantAvatar: assistantIdentity.avatar,
            });
          }

          return nothing;
        },
      )}
    </div>
  `;

  return html`
    <section class="card chat">
      ${renderPreExecutionModal()}
      ${renderPostExecutionModal()}
      ${props.disabledReason ? html`<div class="callout">${props.disabledReason}</div>` : nothing}

      ${props.error ? html`<div class="callout danger">${props.error}</div>` : nothing}

      ${renderCompactionIndicator(props.compactionStatus)}

      ${
        props.focusMode
          ? html`
            <button
              class="chat-focus-exit"
              type="button"
              @click=${props.onToggleFocusMode}
              aria-label="Exit focus mode"
              title="Exit focus mode"
            >
              ${icons.x}
            </button>
          `
          : nothing
      }

      <div class="chat-workspace">
        ${renderSessionSidebar()}
        <div class="chat-workspace__main">
          <div
            class="chat-split-container ${sidebarOpen ? "chat-split-container--open" : ""}"
          >
            <div
              class="chat-main"
              style="flex: ${sidebarOpen ? `0 0 ${splitRatio * 100}%` : "1 1 100%"}"
            >
              ${thread}
            </div>

            ${
              sidebarOpen
                ? html`
                  <resizable-divider
                    .splitRatio=${splitRatio}
                    @resize=${(e: CustomEvent) => props.onSplitRatioChange?.(e.detail.splitRatio)}
                  ></resizable-divider>
                  <div class="chat-sidebar">
                    ${renderMarkdownSidebar({
                      content: props.sidebarContent ?? null,
                      error: props.sidebarError ?? null,
                      onClose: props.onCloseSidebar!,
                      onViewRawText: () => {
                        if (!props.sidebarContent || !props.onOpenSidebar) {
                          return;
                        }
                        props.onOpenSidebar(`\`\`\`\n${props.sidebarContent}\n\`\`\``);
                      },
                    })}
                  </div>
                `
                : nothing
            }
          </div>

      ${
        props.queue.length
          ? html`
            <div class="chat-queue" role="status" aria-live="polite">
              <div class="chat-queue__title">Queued (${props.queue.length})</div>
              <div class="chat-queue__list">
                ${props.queue.map(
                  (item) => html`
                    <div class="chat-queue__item">
                      <div class="chat-queue__text">
                        ${
                          item.text ||
                          (item.attachments?.length ? `Image (${item.attachments.length})` : "")
                        }
                      </div>
                      <button
                        class="btn chat-queue__remove"
                        type="button"
                        aria-label="Remove queued message"
                        @click=${() => props.onQueueRemove(item.id)}
                      >
                        ${icons.x}
                      </button>
                    </div>
                  `,
                )}
              </div>
            </div>
          `
          : nothing
      }

      ${
        props.showNewMessages
          ? html`
            <button
              class="chat-new-messages"
              type="button"
              @click=${props.onScrollToBottom}
            >
              New messages ${icons.arrowDown}
            </button>
          `
          : nothing
      }

      <div class="chat-compose">
        ${renderAttachmentPreview(props)}
        <div class="chat-compose__row">
          <label class="field chat-compose__field">
            <span>Message</span>
            <textarea
              ${ref((el) => el && adjustTextareaHeight(el as HTMLTextAreaElement))}
              .value=${props.draft}
              ?disabled=${!props.connected}
              @keydown=${(e: KeyboardEvent) => {
                if (e.key !== "Enter") {
                  return;
                }
                if (e.isComposing || e.keyCode === 229) {
                  return;
                }
                if (e.shiftKey) {
                  return;
                } // Allow Shift+Enter for line breaks
                if (!props.connected) {
                  return;
                }
                e.preventDefault();
                if (canCompose) {
                  props.onSend();
                }
              }}
              @input=${(e: Event) => {
                const target = e.target as HTMLTextAreaElement;
                adjustTextareaHeight(target);
                props.onDraftChange(target.value);
              }}
              @paste=${(e: ClipboardEvent) => handlePaste(e, props)}
              placeholder=${composePlaceholder}
            ></textarea>
          </label>
          <div class="chat-compose__actions">
            <button
              class="btn"
              ?disabled=${!props.connected || (!canAbort && props.sending)}
              @click=${canAbort ? props.onAbort : props.onNewSession}
            >
              ${canAbort ? "Stop" : "New session"}
            </button>
            <button
              class="btn primary"
              ?disabled=${!props.connected}
              @click=${props.onSend}
            >
              ${isBusy ? "Queue" : "Send"}<kbd class="btn-kbd">↵</kbd>
            </button>
          </div>
        </div>
      </div>
        </div>
      </div>
    </section>
  `;
}

const CHAT_HISTORY_RENDER_LIMIT = 200;

function groupMessages(items: ChatItem[]): Array<ChatItem | MessageGroup> {
  const result: Array<ChatItem | MessageGroup> = [];
  let currentGroup: MessageGroup | null = null;

  for (const item of items) {
    if (item.kind !== "message") {
      if (currentGroup) {
        result.push(currentGroup);
        currentGroup = null;
      }
      result.push(item);
      continue;
    }

    const normalized = normalizeMessage(item.message);
    const role = normalizeRoleForGrouping(normalized.role);
    const timestamp = normalized.timestamp || Date.now();

    if (!currentGroup || currentGroup.role !== role) {
      if (currentGroup) {
        result.push(currentGroup);
      }
      currentGroup = {
        kind: "group",
        key: `group:${role}:${item.key}`,
        role,
        messages: [{ message: item.message, key: item.key }],
        timestamp,
        isStreaming: false,
      };
    } else {
      currentGroup.messages.push({ message: item.message, key: item.key });
    }
  }

  if (currentGroup) {
    result.push(currentGroup);
  }
  return result;
}

function buildChatItems(props: ChatProps): Array<ChatItem | MessageGroup> {
  const items: ChatItem[] = [];
  const history = Array.isArray(props.messages) ? props.messages : [];
  const tools = Array.isArray(props.toolMessages) ? props.toolMessages : [];
  const historyStart = Math.max(0, history.length - CHAT_HISTORY_RENDER_LIMIT);
  if (historyStart > 0) {
    items.push({
      kind: "message",
      key: "chat:history:notice",
      message: {
        role: "system",
        content: `Showing last ${CHAT_HISTORY_RENDER_LIMIT} messages (${historyStart} hidden).`,
        timestamp: Date.now(),
      },
    });
  }
  for (let i = historyStart; i < history.length; i++) {
    const msg = history[i];
    const normalized = normalizeMessage(msg);

    if (!props.showThinking && normalized.role.toLowerCase() === "toolresult") {
      continue;
    }

    items.push({
      kind: "message",
      key: messageKey(msg, i),
      message: msg,
    });
  }
  if (props.showThinking) {
    for (let i = 0; i < tools.length; i++) {
      items.push({
        kind: "message",
        key: messageKey(tools[i], i + history.length),
        message: tools[i],
      });
    }
  }

  if (props.stream !== null) {
    const key = `stream:${props.sessionKey}:${props.streamStartedAt ?? "live"}`;
    if (props.stream.trim().length > 0) {
      items.push({
        kind: "stream",
        key,
        text: props.stream,
        startedAt: props.streamStartedAt ?? Date.now(),
      });
    } else {
      items.push({ kind: "reading-indicator", key });
    }
  }

  return groupMessages(items);
}

function messageKey(message: unknown, index: number): string {
  const m = message as Record<string, unknown>;
  const toolCallId = typeof m.toolCallId === "string" ? m.toolCallId : "";
  if (toolCallId) {
    return `tool:${toolCallId}`;
  }
  const id = typeof m.id === "string" ? m.id : "";
  if (id) {
    return `msg:${id}`;
  }
  const messageId = typeof m.messageId === "string" ? m.messageId : "";
  if (messageId) {
    return `msg:${messageId}`;
  }
  const timestamp = typeof m.timestamp === "number" ? m.timestamp : null;
  const role = typeof m.role === "string" ? m.role : "unknown";
  if (timestamp != null) {
    return `msg:${role}:${timestamp}:${index}`;
  }
  return `msg:${role}:${index}`;
}
