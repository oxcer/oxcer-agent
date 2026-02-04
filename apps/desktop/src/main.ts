import { app, BrowserWindow } from "electron";
import { spawn } from "node:child_process";
import path from "node:path";
import process from "node:process";

const DEFAULT_PORT = 18789;
const DEFAULT_HTTP_ORIGIN = `http://127.0.0.1:${DEFAULT_PORT}`;
const DEFAULT_UI_PATH = "/__openclaw__/canvas/";

type GatewayHandle = {
  proc: ReturnType<typeof spawn>;
};

async function sleep(ms: number) {
  await new Promise((r) => setTimeout(r, ms));
}

async function waitForHttpReady(url: string, opts?: { timeoutMs?: number; intervalMs?: number }) {
  const timeoutMs = opts?.timeoutMs ?? 20_000;
  const intervalMs = opts?.intervalMs ?? 250;
  const start = Date.now();

  while (Date.now() - start < timeoutMs) {
    try {
      const res = await fetch(url, { method: "GET" });
      if (res.ok) {
        return;
      }
    } catch {
      // keep polling
    }
    await sleep(intervalMs);
  }

  throw new Error(`Timed out waiting for gateway HTTP to be ready: ${url}`);
}

async function isGatewayUp(httpOrigin: string): Promise<boolean> {
  try {
    const res = await fetch(`${httpOrigin}/`, { method: "GET" });
    return res.ok;
  } catch {
    return false;
  }
}

function repoRootFromDesktopDir(): string {
  // apps/desktop/dist/main.js -> apps/desktop -> apps -> repo root
  return path.resolve(process.cwd(), "..", "..");
}

function startGatewayChild(repoRoot: string): GatewayHandle {
  const env = {
    ...process.env,
    // Sprint 12: opt-in "app mode" directory layout under ~/.oxcer.
    OXCER_MODE: process.env.OXCER_MODE ?? "app",
  };

  const proc = spawn(process.execPath, ["scripts/run-node.mjs", "gateway", "run"], {
    cwd: repoRoot,
    env,
    stdio: "inherit",
  });

  return { proc };
}

async function createMainWindow(uiUrl: string) {
  const win = new BrowserWindow({
    width: 1200,
    height: 800,
    webPreferences: {
      // Sprint 12 PoC: we’re embedding the existing Control UI.
      // Keep this conservative; if we need Node APIs later, add a preload.
      contextIsolation: true,
      sandbox: true,
    },
  });

  await win.loadURL(uiUrl);
  return win;
}

async function showStartupErrorWindow(message: string) {
  const html = `<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Oxcer Desktop Error</title>
    <style>
      body { font-family: -apple-system, system-ui, Segoe UI, Roboto, Helvetica, Arial, sans-serif; margin: 24px; }
      h1 { font-size: 18px; margin: 0 0 12px; }
      p { margin: 0 0 12px; line-height: 1.4; }
      code, pre { font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace; }
      pre { background: #f5f5f7; padding: 12px; border-radius: 8px; overflow: auto; }
      .hint { color: #444; }
    </style>
  </head>
  <body>
    <h1>Oxcer desktop failed to connect to the gateway.</h1>
    <p class="hint">Check that <code>pnpm oxcer gateway run</code> works in Dev mode, then retry.</p>
    <p><strong>Error:</strong></p>
    <pre>${escapeHtml(message)}</pre>
  </body>
</html>`;

  const win = new BrowserWindow({ width: 640, height: 360 });
  await win.loadURL(`data:text/html,${encodeURIComponent(html)}`);
  return win;
}

function escapeHtml(input: string): string {
  return input
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}

async function main() {
  await app.whenReady();

  let gateway: GatewayHandle | null = null;
  try {
    const httpOrigin = process.env.OXCER_APP_HTTP_ORIGIN?.trim() || DEFAULT_HTTP_ORIGIN;
    const uiPath = process.env.OXCER_APP_UI_PATH?.trim() || DEFAULT_UI_PATH;
    const uiUrl = `${httpOrigin}${uiPath}`;

    const alreadyUp = await isGatewayUp(httpOrigin);
    if (!alreadyUp) {
      const repoRoot = repoRootFromDesktopDir();
      gateway = startGatewayChild(repoRoot);
    }

    await waitForHttpReady(`${httpOrigin}/`);
    await createMainWindow(uiUrl);

    app.on("before-quit", () => {
      if (!gateway) {
        return;
      }
      try {
        gateway.proc.kill("SIGTERM");
      } catch {
        // best-effort
      }
    });
  } catch (err) {
    const message = String((err as Error)?.message ?? err);
    process.stderr.write(`oxcer-desktop: failed to start: ${message}\n`);
    await showStartupErrorWindow(message);
  }
}

void main().catch((err) => {
  process.stderr.write(`oxcer-desktop: failed to start: ${String(err)}\n`);
  process.exit(1);
});

