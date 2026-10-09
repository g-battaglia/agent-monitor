/** tmux-agent-monitor presence v1: metadata only, no prompts or session mutations. */
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { promises as fs, existsSync } from "node:fs";
import { join, isAbsolute } from "node:path";
import { homedir } from "node:os";
import { randomUUID } from "node:crypto";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

const exec = promisify(execFile);
const legacyRoot = join(homedir(), ".local/state/agent-monitor");
const canonicalRoot = join(homedir(), ".local/state/tmux-agent-monitor");
const defaultRoot = existsSync(join(canonicalRoot, "sessions.db")) ? canonicalRoot
  : existsSync(legacyRoot) ? legacyRoot : canonicalRoot;
const configuredRoot = process.env.TMUX_AGENT_MONITOR_HOME || process.env.AGENT_MONITOR_HOME || defaultRoot;
const root = configuredRoot === "~" ? homedir()
  : configuredRoot.startsWith("~/") ? join(homedir(), configuredRoot.slice(2)) : configuredRoot;

async function privateDirectory(path: string) {
  await fs.mkdir(path, { recursive: true, mode: 0o700 });
  const stat = await fs.lstat(path);
  if (stat.isSymbolicLink() || !stat.isDirectory() || stat.uid !== process.getuid?.() || (stat.mode & 0o077)) {
    throw new Error("Unsafe presence directory");
  }
}

export default function presence(pi: ExtensionAPI) {
  let timer: ReturnType<typeof setInterval> | undefined;
  let context: ExtensionContext | undefined;
  let generation = 0;
  let nonce = "";
  let processStart = "";
  let queued: object | undefined;
  let running: Promise<void> | undefined;
  const dir = join(root, "presence");
  const file = join(dir, `${process.pid}.json`);

  async function flush() {
    while (queued) {
      const record = queued;
      queued = undefined;
      const temporary = join(dir, `.${process.pid}-${randomUUID()}.tmp`);
      try {
        if (!isAbsolute(root)) return;
        await privateDirectory(root);
        await privateDirectory(dir);
        // Atomic replacement never follows a destination symlink.
        await fs.writeFile(temporary, JSON.stringify(record), { mode: 0o600, flag: "wx" });
        await fs.rename(temporary, file);
      } catch { /* Reporting must never break the host session. */ }
      finally { await fs.unlink(temporary).catch(() => {}); }
    }
  }
  function publish() {
    if (!context || !processStart) return;
    const manager = context.sessionManager;
    const tmux = process.env.TMUX?.replace(/,[^,]*,[^,]*$/, "");
    queued = {
      version: 1, nonce, generation, pid: process.pid, process_start: processStart,
      native_id: manager.getSessionId(), file: manager.getSessionFile() ?? null,
      cwd: manager.getCwd(), name: manager.getSessionName() ?? "",
      leaf: manager.getLeafId(), socket: tmux ?? null, pane: process.env.TMUX_PANE ?? null,
      seen: Date.now(),
    };
    if (!running) {
      running = flush().finally(() => { running = undefined; if (queued) publish(); });
    }
  }
  async function stop() {
    if (timer) clearInterval(timer);
    timer = undefined;
    context = undefined;
    queued = undefined;
    await running;
    try {
      const raw = await fs.readFile(file, "utf8");
      if (JSON.parse(raw).nonce === nonce) await fs.unlink(file);
    } catch { /* Already removed, unavailable, or replaced by another runtime. */ }
  }
  pi.on("session_start", async (_event, ctx) => {
    await stop();
    generation += 1;
    nonce = randomUUID();
    try {
      const { stdout } = await exec("ps", ["-p", String(process.pid), "-o", "lstart="], {
        env: { ...process.env, LC_ALL: "C", TZ: "UTC" }, timeout: 1000, maxBuffer: 4096,
      });
      processStart = stdout.trim().replace(/\s+/g, " ");
      context = ctx;
      publish();
      timer = setInterval(publish, 2000);
      timer.unref();
    } catch { /* Pi remains usable without presence reporting. */ }
  });
  pi.on("session_info_changed", () => publish());
  pi.on("session_tree", () => publish());
  pi.on("session_shutdown", () => stop());
}
