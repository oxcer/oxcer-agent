import { spawn, exec } from "node:child_process";
import process from "node:process";

const DEFAULT_PORT = 18789;
const DEFAULT_HTTP_ORIGIN = `http://127.0.0.1:${DEFAULT_PORT}`;
const DEFAULT_UI_PATH = "/__openclaw__/canvas/";

function openUrl(url: string) {
  // Sprint 12: macOS-only PoC.
  exec(`open ${JSON.stringify(url)}`);
}

async function isGatewayUp(httpOrigin: string): Promise<boolean> {
  try {
    const res = await fetch(`${httpOrigin}/`, { method: "GET" });
    return res.ok;
  } catch {
    return false;
  }
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
    await new Promise((r) => setTimeout(r, intervalMs));
  }

  throw new Error(`Timed out waiting for gateway HTTP to be ready: ${url}`);
}

function startGatewayChild() {
  const env = {
    ...process.env,
    // Sprint 12: opt-in "app mode" directory layout under ~/.oxcer.
    OXCER_MODE: process.env.OXCER_MODE ?? "app",
  };

  const child = spawn(process.execPath, ["scripts/run-node.mjs", "gateway", "run"], {
    cwd: process.cwd(),
    env,
    stdio: "inherit",
  });

  child.on("exit", (code, signal) => {
    if (signal) {
      process.exit(1);
    }
    process.exit(code ?? 1);
  });

  return child;
}

async function main() {
  const httpOrigin = process.env.OXCER_APP_HTTP_ORIGIN?.trim() || DEFAULT_HTTP_ORIGIN;
  const uiPath = process.env.OXCER_APP_UI_PATH?.trim() || DEFAULT_UI_PATH;
  const uiUrl = `${httpOrigin}${uiPath}`;

  const alreadyUp = await isGatewayUp(httpOrigin);
  let spawnMode: "spawned" | "reused" = "reused";
  if (!alreadyUp) {
    spawnMode = "spawned";
    startGatewayChild();
    await waitForHttpReady(`${httpOrigin}/`);
  } else {
    process.stderr.write("Oxcer app launcher: gateway already running, skipping spawn\n");
  }
  openUrl(uiUrl);

  // Keep the parent process alive while the gateway child runs (stdio is inherited).
  // We intentionally keep process supervision minimal in Sprint 12.
  process.stderr.write(
    `Oxcer app launcher: gateway ready at ${httpOrigin} (${spawnMode}), UI opened at ${uiUrl}\n`,
  );
}

await main();

