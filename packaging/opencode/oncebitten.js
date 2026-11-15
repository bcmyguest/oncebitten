// oncebitten-managed: written by `oncebitten hooks install opencode`; remove with `hooks uninstall`.
// Forwards each prompt to `oncebitten capture --host opencode`. Fails open.
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

export default {
  id: "oncebitten",
  async setup(ctx) {
    await ctx.session.hook("prompt", async (event) => {
      forward({ ...event, hook_event_name: "prompt", directory: ctx.location?.directory });
    });
  },
};
