// Entry point when pi installs this repository as a package (`pi install git:github.com/bcmyguest/oncebitten`).
// Same as oncebitten.js, but runs the release launcher shipped in the repository. Fails open.
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const COMMAND = `sh "${fileURLToPath(new URL("../claude/bin/oncebitten", import.meta.url))}" capture --host pi`;
const TIMEOUT_MS = 10000;

function forward(payload) {
  if (process.env.ONCEBITTEN_TRIAGE === "1") return;
  try {
    const child = spawn(COMMAND, { shell: true, stdio: ["pipe", "ignore", "ignore"], windowsHide: true });
    const timer = setTimeout(() => child.kill(), TIMEOUT_MS);
    child.on("error", () => clearTimeout(timer));
    child.on("close", () => clearTimeout(timer));
    child.stdin.on("error", () => {});
    child.stdin.end(JSON.stringify(payload));
  } catch {
    // a capture failure never reaches the agent
  }
}

export default function oncebitten(pi) {
  pi.on("before_agent_start", async (event, ctx) => {
    forward({
      hook_event_name: "before_agent_start",
      session_id: ctx.sessionManager.getSessionId(),
      cwd: ctx.cwd,
      prompt: String(event.prompt ?? ""),
    });
  });
  pi.on("tool_result", async (event, ctx) => {
    forward({
      hook_event_name: "tool_result",
      session_id: ctx.sessionManager.getSessionId(),
      cwd: ctx.cwd,
      toolName: event.toolName,
      input: event.input,
      isError: event.isError === true,
    });
  });
}
