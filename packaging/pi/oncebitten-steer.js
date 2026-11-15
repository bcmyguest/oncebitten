// oncebitten-managed: written by `oncebitten hooks install pi`; remove with `hooks uninstall`.
// Asks `oncebitten steer --host pi` about each tool call and prompt. Fails open.
import { spawn } from "node:child_process";

const COMMAND = __ONCEBITTEN_COMMAND__;
const TIMEOUT_MS = 10000;
const MESSAGE_TYPE = "oncebitten";

function ask(payload) {
  if (process.env.ONCEBITTEN_TRIAGE === "1") return Promise.resolve(null);
  return new Promise((resolve) => {
    try {
      const child = spawn(COMMAND, { shell: true, stdio: ["pipe", "pipe", "ignore"], windowsHide: true });
      let out = "";
      const timer = setTimeout(() => {
        child.kill();
        resolve(null);
      }, TIMEOUT_MS);
      child.stdout.on("data", (chunk) => {
        out += chunk;
      });
      child.on("error", () => {
        clearTimeout(timer);
        resolve(null);
      });
      child.on("close", () => {
        clearTimeout(timer);
        try {
          resolve(out.trim() ? JSON.parse(out) : null);
        } catch {
          resolve(null);
        }
      });
      child.stdin.on("error", () => {});
      child.stdin.end(JSON.stringify(payload));
    } catch {
      // a steer failure never reaches the agent
      resolve(null);
    }
  });
}

export default function oncebittenSteer(pi) {
  pi.on("tool_call", async (event, ctx) => {
    const answer = await ask({
      hook_event_name: "tool_call",
      cwd: ctx.cwd,
      toolName: event.toolName,
      input: event.input,
    });
    if (answer?.deny) return { block: true, reason: answer.deny };
    if (answer?.context) {
      // Lands after this turn's tool calls, before the next model call.
      pi.sendMessage({ customType: MESSAGE_TYPE, content: answer.context, display: false }, { deliverAs: "steer" });
    }
  });
  pi.on("before_agent_start", async (event, ctx) => {
    const answer = await ask({
      hook_event_name: "before_agent_start",
      cwd: ctx.cwd,
      prompt: String(event.prompt ?? ""),
    });
    if (answer?.context) {
      return { message: { customType: MESSAGE_TYPE, content: answer.context, display: false } };
    }
  });
}
