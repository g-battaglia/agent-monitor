import { test, expect } from "bun:test";
import { mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Test the actual observer with a simulated Pi host, never launch a provider.
test("startup/name/resume/shutdown write metadata only and clean up", async () => {
  const root = await mkdtemp(join(tmpdir(), "am-presence-"));
  const previous = process.env.AGENT_MONITOR_HOME;
  process.env.AGENT_MONITOR_HOME = root;
  const { default: extension } = await import("./session-presence.ts");
  const handlers = new Map<string, Function>();
  let session = "native-one", name = "first", leaf = "one";
  const ctx = { sessionManager: {
    getSessionId: () => session, getSessionFile: () => undefined,
    getCwd: () => "/fixture/project", getSessionName: () => name, getLeafId: () => leaf,
  }};
  extension({ on: (event: string, handler: Function) => { handlers.set(event, handler); } } as any);
  const file = join(root, "presence", `${process.pid}.json`);
  async function record(wanted: string) {
    for (let i = 0; i < 100; i++) {
      try { const data = JSON.parse(await readFile(file, "utf8")); if (data.native_id === wanted) return data; } catch {}
      await Bun.sleep(10);
    }
    throw new Error("presence was not published");
  }
  try {
    await handlers.get("session_start")!({ reason: "startup" }, ctx);
    const first = await record(session);
    expect(first.name).toBe("first"); expect(first.file).toBeNull();
    expect(Object.keys(first).sort()).toEqual(["version","nonce","generation","pid","process_start","native_id","file","cwd","name","leaf","socket","pane","seen"].sort());
    expect((await stat(file)).mode & 0o777).toBe(0o600);
    await handlers.get("session_shutdown")!({ reason: "resume" }, ctx);
    session = "native-two"; name = "renamed"; leaf = "two";
    await handlers.get("session_start")!({ reason: "resume" }, ctx);
    const second = await record(session);
    expect(second.nonce).not.toBe(first.nonce); expect(second.generation).toBeGreaterThan(first.generation);
    name = "updated";
    await handlers.get("session_info_changed")!({}, ctx);
    for (let i = 0; i < 100; i++) { if (JSON.parse(await readFile(file,"utf8")).name === name) break; await Bun.sleep(10); }
    expect(JSON.parse(await readFile(file,"utf8")).name).toBe(name);
    await handlers.get("session_shutdown")!({ reason: "quit" }, ctx);
    expect(await readFile(file).then(() => true, () => false)).toBe(false);
    // Unavailable/unsafe storage must not interrupt the host lifecycle.
    await rm(join(root,"presence"), { recursive: true });
    await writeFile(join(root,"presence"), "not a directory");
    await handlers.get("session_start")!({ reason: "reload" }, ctx);
    await handlers.get("session_shutdown")!({ reason: "quit" }, ctx);
  } finally {
    await handlers.get("session_shutdown")!({}, ctx);
    await rm(root, { recursive: true, force: true });
    if (previous === undefined) delete process.env.AGENT_MONITOR_HOME; else process.env.AGENT_MONITOR_HOME = previous;
  }
});
