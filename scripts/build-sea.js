import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const buildDir = join(root, "build");
const runtimeDir = join(buildDir, "runtime");
const distDir = join(root, "dist");
const bundle = join(buildDir, "qqnt-export.cjs");
const config = join(buildDir, "sea-config.json");
const blob = join(buildDir, "qqnt-export.blob");
const output = join(distDir, "qqnt-export.exe");
const postject = join(root, "node_modules", "postject", "dist", "cli.js");

if (process.platform !== "win32") {
  throw new Error("当前 SEA 发布流程只构建 Windows x64 版本");
}

const assets = {
  "qqnt-export-core.exe": join(runtimeDir, "qqnt-export-core.exe"),
  "sqlcipher.exe": join(runtimeDir, "sqlcipher", "sqlcipher.exe"),
  "libcrypto-3-x64.dll": join(runtimeDir, "sqlcipher", "libcrypto-3-x64.dll"),
  "zlib1.dll": join(runtimeDir, "sqlcipher", "zlib1.dll"),
};
for (const path of Object.values(assets)) {
  if (!existsSync(path)) {
    throw new Error(`缺少发布资源: ${path}`);
  }
}

await build({
  entryPoints: [join(root, "bin", "qqnt-export.js")],
  outfile: bundle,
  bundle: true,
  format: "cjs",
  platform: "node",
  target: "node24",
  legalComments: "none",
  logLevel: "info",
  logOverride: {
    "empty-import-meta": "silent",
  },
});

writeFileSync(
  config,
  `${JSON.stringify(
    {
      main: bundle,
      output: blob,
      disableExperimentalSEAWarning: true,
      useSnapshot: false,
      useCodeCache: true,
      assets,
    },
    null,
    2,
  )}\n`,
);

run(process.execPath, ["--experimental-sea-config", config]);

rmSync(distDir, { recursive: true, force: true });
mkdirSync(distDir, { recursive: true });
copyFileSync(process.execPath, output);
stripAuthenticodeSignature(output);

run(process.execPath, [
  postject,
  output,
  "NODE_SEA_BLOB",
  blob,
  "--sentinel-fuse",
  "NODE_SEA_FUSE_fce680ab2cc467b6e072b8b5df1996b2",
]);

run(output, ["--version"]);
run(output, ["-h"]);
run(output, ["doctor", "--json"]);

const hash = createHash("sha256").update(readFileSync(output)).digest("hex");
writeFileSync(join(distDir, "SHA256SUMS.txt"), `${hash}  qqnt-export.exe\n`);
writeFileSync(
  join(distDir, "BUILD_INFO.txt"),
  `node=${process.version}\nplatform=win32-x64\nbytes=${readFileSync(output).length}\nsha256=${hash}\n`,
);
copyFileSync(join(root, "LICENSE"), join(distDir, "LICENSE"));
copyFileSync(
  join(root, "THIRD_PARTY_NOTICES.md"),
  join(distDir, "THIRD_PARTY_NOTICES.md"),
);
copyFileSync(
  join(dirname(process.execPath), "LICENSE"),
  join(distDir, "NODE_LICENSE.txt"),
);
copyFileSync(
  join(root, "vendor", "sqlcipher", "SQLCIPHER_LICENSE.txt"),
  join(distDir, "SQLCIPHER_LICENSE.txt"),
);
copyFileSync(
  join(root, "vendor", "sqlcipher", "SQLCIPHER_SQLITE_LICENSE.md"),
  join(distDir, "SQLCIPHER_SQLITE_LICENSE.md"),
);
process.stdout.write(`SEA: ${output}\n`);

function run(command, args) {
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit" });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`命令失败 (${result.status}): ${command} ${args.join(" ")}`);
  }
}

function stripAuthenticodeSignature(path) {
  const data = readFileSync(path);
  if (data.readUInt16LE(0) !== 0x5a4d) {
    throw new Error("Node 可执行文件不是有效 PE 文件");
  }
  const peOffset = data.readUInt32LE(0x3c);
  if (data.readUInt32LE(peOffset) !== 0x00004550) {
    throw new Error("Node 可执行文件 PE 头无效");
  }
  const optionalHeader = peOffset + 24;
  if (data.readUInt16LE(optionalHeader) !== 0x20b) {
    throw new Error("当前发布流程只支持 PE32+");
  }

  const certificateDirectory = optionalHeader + 112 + 4 * 8;
  const certificateOffset = data.readUInt32LE(certificateDirectory);
  const certificateSize = data.readUInt32LE(certificateDirectory + 4);
  if (certificateOffset === 0 || certificateSize === 0) {
    return;
  }
  if (certificateOffset >= data.length) {
    throw new Error("Node 可执行文件证书表位置无效");
  }

  data.fill(0, certificateDirectory, certificateDirectory + 8);
  writeFileSync(path, data.subarray(0, certificateOffset));
}
