// oncebitten-managed: written by `oncebitten hooks install opencode`; remove with `hooks uninstall`.
// Asks `oncebitten steer --host opencode` about each tool call and prompt. Fails open.
import { spawn } from "node:child_process";

const COMMAND = __ONCEBITTEN_COMMAND__;
const TIMEOUT_MS = 10000;

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

export default {
  id: "oncebitten-steer",
  async setup(ctx) {
    await ctx.tool.hook("execute.before", async (event) => {
      const answer = await ask({
        hook_event_name: "tool.execute.before",
        tool: event.tool,
        input: event.input,
        directory: ctx.location?.directory,
      });
      if (answer?.deny) throw new Error(answer.deny);
    });
    await ctx.session.hook("prompt", async (event) => {
      const answer = await ask({ ...event, hook_event_name: "prompt", directory: ctx.location?.directory });
      if (answer?.context && event.prompt) event.prompt.text = event.prompt.text + "\n\n" + answer.context;
    });
  },
};
