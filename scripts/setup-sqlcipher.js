import { createHash } from "node:crypto";
import { createWriteStream, existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const vendor = join(root, "vendor", "sqlcipher");
const executable = join(vendor, "sqlcipher.exe");
const archive = join(root, "vendor", ".sqlcipher-download.zip");
const url =
  "https://github.com/ShintoKosei/sqlcipher-windows-builds/releases/download/" +
  "sqlcipher-v4.19.0-windows-x64-ucrt/sqlcipher-v4.19.0-windows-x64-ucrt.zip";
const expectedSha256 =
  "bee3d197c24973a36c1df067249b791d9df4562c4595934a4ad79ae30e2a92a6";

if (process.platform !== "win32") {
  process.stdout.write("SQLCipher Windows helper: skipped on non-Windows platform.\n");
  process.exit(0);
}

if (existsSync(executable)) {
  process.stdout.write(`SQLCipher: ${executable}\n`);
  process.exit(0);
}

mkdirSync(dirname(archive), { recursive: true });
process.stdout.write("Downloading pinned SQLCipher Windows build...\n");
const response = await fetch(url, { redirect: "follow" });
if (!response.ok || !response.body) {
  throw new Error(`SQLCipher 下载失败: HTTP ${response.status}`);
}
await pipeline(Readable.fromWeb(response.body), createWriteStream(archive));

const digest = createHash("sha256").update(readFileSync(archive)).digest("hex");
if (digest !== expectedSha256) {
  rmSync(archive, { force: true });
  throw new Error(`SQLCipher 下载校验失败: ${digest}`);
}

mkdirSync(vendor, { recursive: true });
const unzip = spawnSync(
  "powershell.exe",
  [
    "-NoProfile",
    "-Command",
    `Expand-Archive -LiteralPath '${archive}' -DestinationPath '${vendor}' -Force`,
  ],
  { stdio: "inherit" },
);
rmSync(archive, { force: true });
if (unzip.status !== 0) {
  throw new Error("无法解压 SQLCipher 构建包。请确认 PowerShell Expand-Archive 可用。");
}
if (!existsSync(executable)) {
  throw new Error(`SQLCipher 压缩包结构异常，未找到 ${executable}`);
}
process.stdout.write(`SQLCipher: ${executable}\n`);
