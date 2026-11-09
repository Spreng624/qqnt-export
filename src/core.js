import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { spawn } from "node:child_process";
import { bundledCoreDirectory, bundledCorePath } from "./runtime.js";

const projectRoot = dirname(dirname(resolve(process.argv[1] || process.execPath)));

export function corePath() {
  const executable = process.platform === "win32" ? "qqnt-export-core.exe" : "qqnt-export-core";
  const candidates = [
    process.env.QQNT_EXPORT_CORE,
    bundledCorePath(),
    join(projectRoot, "dist", executable),
    join(projectRoot, "core", "target", "release", executable),
    join(projectRoot, "core", "target", "debug", executable),
  ].filter(Boolean);

  const found = candidates.find((candidate) => existsSync(candidate));
  if (!found) {
    throw new Error("未找到 Rust 核心。请先运行 npm run build。");
  }
  return found;
}

export function runCore(args, { stdinText = null, quiet = false } = {}) {
  const executable = corePath();
  return new Promise((resolve, reject) => {
    const child = spawn(executable, args, {
      cwd: bundledCoreDirectory() || dirname(executable),
      windowsHide: true,
      stdio: ["pipe", "pipe", quiet ? "pipe" : "inherit"],
    });

    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    if (quiet && child.stderr) {
      child.stderr.setEncoding("utf8");
      child.stderr.on("data", (chunk) => {
        stderr += chunk;
      });
    }

    child.on("error", (error) => reject(error));
    child.stdin.on("error", (error) => {
      if (error?.code !== "EPIPE") {
        stderr += `${error.message}\n`;
      }
    });
    child.on("close", (code, signal) => {
      if (code !== 0) {
        const detail = stderr.trim() || stdout.trim() || signal || `退出码 ${code}`;
        reject(new Error(detail));
        return;
      }
      try {
        resolve(stdout.trim() ? JSON.parse(stdout) : null);
      } catch {
        reject(new Error(`Rust 核心返回了无效 JSON: ${stdout.slice(0, 500)}`));
      }
    });

    if (stdinText !== null) {
      child.stdin.write(stdinText);
    }
    child.stdin.end();
  });
}
