import readline from "node:readline/promises";
import { stdin as input, stdout as output } from "node:process";

export function isInteractive() {
  return Boolean(input.isTTY && output.isTTY);
}

export async function ask(question, { defaultValue = "" } = {}) {
  const rl = readline.createInterface({ input, output });
  try {
    const suffix = defaultValue ? ` [${defaultValue}]` : "";
    const answer = (await rl.question(`${question}${suffix}: `)).trim();
    return answer || defaultValue;
  } finally {
    rl.close();
  }
}

export async function confirm(question, defaultYes = false) {
  const hint = defaultYes ? "Y/n" : "y/N";
  const answer = (await ask(`${question} [${hint}]`)).toLowerCase();
  if (!answer) {
    return defaultYes;
  }
  return answer === "y" || answer === "yes" || answer === "是";
}

export function formatTime(timestamp) {
  if (!timestamp) {
    return "时间未知";
  }
  return new Date(timestamp * 1000).toLocaleString("zh-CN", { hour12: false });
}
