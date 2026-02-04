export type SessionProfile = "default";

export type SessionSummary = {
  sessionKey: string;
  title?: string;
  favorite?: boolean;
  profile: SessionProfile;
  updatedAt: string;
  // Local-only UI status, not part of the gateway schema
  localStatus?: "draft" | "error";
};

export type GuardrailEvent = {
  id: string;
  timestamp: number;
  type: "action" | "result";
  decision: "allow" | "deny" | "needs_human";
  summary: string;
  tool: string;
  status: "pending_review" | "resolved" | "auto_resolved";
};

export type ActionRecord = {
  id: string;
  sessionKey: string;
  timestamp: string;
  tool: string;
  summary: string;
  planned: boolean;
  outcome?: "success" | "failed" | "blocked" | "skipped";
  failureReason?: string;
  riskLevel: "low" | "medium" | "high" | "critical";
};

export type ChatMessage = {
  role: string;
  content: unknown;
  timestamp?: number;
};

export type GatewayRequestResult<R> = {
  ok: boolean;
  payload?: R;
  error?: string;
};

type GatewayResponseFrame = {
  type: "res";
  id: number;
  ok: boolean;
  payload?: unknown;
  error?: { code?: string; message?: string; details?: unknown } | string;
};

type PendingHandler = (frame: GatewayResponseFrame) => void;

export class GatewayClient {
  private socket: WebSocket | null = null;
  private url: string;
  private token?: string;

  // id=0 is reserved for the initial "connect" call
  private nextId = 1;
  private pending = new Map<number, PendingHandler>();

  private connectPromise: Promise<void> | null = null;

  constructor(opts: { url: string; token?: string }) {
    this.url = opts.url;
    this.token = opts.token;
  }

  /**
   * Open the WebSocket and perform the initial "connect" RPC (id=0).
   * Resolves only when connect returns ok: true, rejects otherwise.
   */
  private openAndConnect(): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      const url = new URL(this.url);
      if (this.token) {
        // Optional: pass token via query as well; main auth is in connect params.auth
        url.searchParams.set("token", this.token);
      }

      const ws = new WebSocket(url);
      this.socket = ws;

      let connectSettled = false;

      ws.onopen = () => {
        // First frame MUST be "connect" with id=0
        const connectId = 0;

        this.pending.set(connectId, (frame: GatewayResponseFrame) => {
          connectSettled = true;
          if (frame.ok) {
            resolve();
          } else {
            const msg =
              typeof frame.error === "string"
                ? frame.error
                : frame.error?.message ?? "connect failed";
            reject(new Error(msg));
          }
        });

        const params: Record<string, unknown> = {
          role: "operator",
        };
        if (this.token) {
          params.auth = { token: this.token };
        }

        const connectFrame = {
          type: "req" as const,
          id: connectId,
          method: "connect",
          params,
        };

        ws.send(JSON.stringify(connectFrame));
      };

      ws.onmessage = (ev: MessageEvent) => this.onMessage(ev);

      ws.onerror = () => {
        if (!connectSettled) {
          reject(new Error("WebSocket connection error"));
        }
        // Any pending requests will be flushed in onclose
      };

      ws.onclose = (ev) => {
        const reason = ev.reason || "connection closed";
        const errFrame: GatewayResponseFrame = {
          type: "res",
          id: -1,
          ok: false,
          error: { code: "CLOSED", message: reason },
        };

        // Reject all pending requests (including connect if still pending)
        for (const [, handler] of this.pending) {
          handler(errFrame);
        }
        this.pending.clear();
        this.socket = null;

        if (!connectSettled) {
          reject(new Error(`connection closed before connect completed: ${reason}`));
        }

        // Allow future calls to attempt a fresh connection
        this.connectPromise = null;
        this.nextId = 1;
      };
    });
  }

  /**
   * Ensure we have an open socket AND a successful connect handshake.
   */
  private async ensureConnected(): Promise<void> {
    if (!this.connectPromise) {
      this.connectPromise = this.openAndConnect();
    }
    // If this rejects, callers will see the error and no requests will be sent
    await this.connectPromise;
  }

  /**
   * Handle incoming "res" frames and dispatch to the appropriate pending handler.
   */
  private onMessage(ev: MessageEvent) {
    try {
      const raw = String(ev.data ?? "");
      const parsed = JSON.parse(raw) as { type?: unknown; id?: unknown };

      if (parsed.type !== "res" || typeof parsed.id !== "number") {
        return;
      }

      const frame = parsed as GatewayResponseFrame;
      const handler = this.pending.get(frame.id);
      if (!handler) {
        return;
      }
      this.pending.delete(frame.id);
      handler(frame);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("gateway: failed to handle message", err);
    }
  }

  /**
   * Generic RPC request helper that waits for connectPromise before sending.
   */
  async request<R = unknown, P = unknown>(
    method: string,
    params?: P,
  ): Promise<GatewayRequestResult<R>> {
    try {
      await this.ensureConnected();
    } catch (err) {
      return { ok: false, error: (err as Error).message || String(err) };
    }

    const ws = this.socket;
    if (!ws || ws.readyState !== WebSocket.OPEN) {
      return { ok: false, error: "socket not open" };
    }

    const id = this.nextId++;
    const frame = { type: "req" as const, id, method, params };

    return new Promise<GatewayRequestResult<R>>((resolve) => {
      this.pending.set(id, (resFrame: GatewayResponseFrame) => {
        if (resFrame.ok) {
          resolve({
            ok: true,
            payload: resFrame.payload as R | undefined,
          });
        } else {
          const msg =
            typeof resFrame.error === "string"
              ? resFrame.error
              : resFrame.error?.message ?? "request failed";
          resolve({ ok: false, error: msg });
        }
      });

      ws.send(JSON.stringify(frame));
    });
  }

  // High-level helpers used by the chat UI

  async listSessions(): Promise<GatewayRequestResult<SessionSummary[]>> {
    const res = await this.request<{ sessions?: SessionSummary[] }>("session.list", {
      limit: 50,
    });
    if (!res.ok) return { ok: false, error: res.error };
    return { ok: true, payload: res.payload?.sessions ?? [] };
  }

  async createSession(): Promise<GatewayRequestResult<SessionSummary>> {
    const res = await this.request<{ session?: SessionSummary }>("session.create", {});
    if (!res.ok || !res.payload?.session) {
      // eslint-disable-next-line no-console
      console.error("createSession failed", res);
      return { ok: false, error: res.error ?? "missing session in response" };
    }
    return { ok: true, payload: res.payload.session };
  }

  async updateSession(
    sessionKey: string,
    patch: Partial<Pick<SessionSummary, "title" | "favorite" | "profile">>,
  ): Promise<GatewayRequestResult<SessionSummary>> {
    const res = await this.request<{ session?: SessionSummary }>("session.update", {
      sessionKey,
      patch,
    });
    if (!res.ok) return { ok: false, error: res.error };
    if (!res.payload?.session) return { ok: false, error: "missing session in response" };
    return { ok: true, payload: res.payload.session };
  }

  async listGuardrailEvents(sessionKey: string): Promise<GatewayRequestResult<GuardrailEvent[]>> {
    const res = await this.request<{ events?: GuardrailEvent[] }>("guardrails.events.list", {
      sessionKey,
      limit: 50,
    });
    if (!res.ok) return { ok: false, error: res.error };
    return { ok: true, payload: res.payload?.events ?? [] };
  }

  async listActions(sessionKey: string): Promise<GatewayRequestResult<ActionRecord[]>> {
    const res = await this.request<{ actions?: ActionRecord[] }>("session.actions.list", {
      sessionKey,
      onlyExecuted: true,
      limit: 100,
    });
    if (!res.ok) return { ok: false, error: res.error };
    return { ok: true, payload: res.payload?.actions ?? [] };
  }

  async getPreExecutionReport(sessionKey: string) {
    return this.request(
      "session.report.preExecution",
      { sessionKey } as { sessionKey: string },
    );
  }

  async getPostExecutionReport(sessionKey: string) {
    return this.request(
      "session.report.postExecution",
      { sessionKey } as { sessionKey: string },
    );
  }

  async listChatHistory(
    sessionKey: string,
  ): Promise<GatewayRequestResult<{ messages: ChatMessage[] }>> {
    const res = await this.request<{ messages?: ChatMessage[] }>("chat.history", {
      sessionKey,
      limit: 200,
    });
    if (!res.ok) return { ok: false, error: res.error };
    return { ok: true, payload: { messages: res.payload?.messages ?? [] } };
  }

  async sendChatMessage(
    sessionKey: string,
    message: string,
  ): Promise<GatewayRequestResult<void>> {
    if (!message.trim()) {
      return { ok: false, error: "message is empty" };
    }
    const res = await this.request<
      unknown,
      { sessionKey: string; message: string; deliver: boolean }
    >("chat.send", {
      sessionKey,
      message,
      deliver: false,
    });
    if (!res.ok) {
      return { ok: false, error: res.error ?? "chat.send failed" };
    }
    return { ok: true };
  }
}

