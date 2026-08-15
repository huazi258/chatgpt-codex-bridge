import { spawn } from "node:child_process";
import readline from "node:readline";

const command = process.platform === "win32" ? "D:\\node\\global\\codex.cmd" : "codex";
const proc = spawn(command, ["app-server"], {
  shell: process.platform === "win32",
  stdio: ["pipe", "pipe", "pipe"],
});

let threadId;
let finalMessage = "";
let settled = false;

function send(message) {
  proc.stdin.write(`${JSON.stringify(message)}\n`);
}

function finish(exitCode, message) {
  if (settled) return;
  settled = true;
  console.log(message);
  proc.kill();
  process.exit(exitCode);
}

const timeout = setTimeout(() => {
  finish(1, "SMOKE_FAIL: timed out waiting for turn/completed");
}, 120_000);

readline.createInterface({ input: proc.stdout }).on("line", (line) => {
  let message;
  try {
    message = JSON.parse(line);
  } catch {
    return;
  }

  if (message.id === 1 && message.result) {
    send({ method: "initialized", params: {} });
    send({ method: "thread/start", id: 2, params: {} });
    return;
  }

  if (message.id === 2 && message.result?.thread?.id) {
    threadId = message.result.thread.id;
    send({
      method: "turn/start",
      id: 3,
      params: {
        threadId,
        input: [
          {
            type: "text",
            text: "Reply with exactly APP_SERVER_SMOKE_OK. Do not run commands, inspect files, or modify anything.",
          },
        ],
      },
    });
    return;
  }

  if (message.method === "item/agentMessage/delta") {
    finalMessage += message.params?.delta ?? "";
    return;
  }

  if (message.method === "item/completed" && message.params?.item?.type === "agentMessage") {
    finalMessage = message.params.item.text ?? finalMessage;
    return;
  }

  if (message.method === "turn/completed") {
    clearTimeout(timeout);
    const status = message.params?.turn?.status;
    if (status === "completed" && finalMessage.includes("APP_SERVER_SMOKE_OK")) {
      finish(0, `SMOKE_OK: thread=${threadId}; response=${finalMessage.trim()}`);
    }
    finish(1, `SMOKE_FAIL: status=${status}; response=${finalMessage.trim() || "(none)"}`);
  }
});

proc.stderr.on("data", (chunk) => {
  process.stderr.write(chunk);
});

proc.on("error", (error) => finish(1, `SMOKE_FAIL: ${error.message}`));
proc.on("exit", (code) => {
  if (!settled) finish(1, `SMOKE_FAIL: app-server exited early with code ${code}`);
});

send({
  method: "initialize",
  id: 1,
  params: {
    clientInfo: {
      name: "chatgpt-codex-middleware-smoke",
      title: "ChatGPT Codex Middleware Smoke Test",
      version: "0.1.0",
    },
  },
});
