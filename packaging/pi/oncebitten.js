// oncebitten-managed: written by `oncebitten hooks install pi`; remove with `hooks uninstall`.
// Forwards prompts and tool results to `oncebitten capture --host pi`. Fails open.
import { spawn } from "node:child_process";

const COMMAND = __ONCEBITTEN_COMMAND__;
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
