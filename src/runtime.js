import * as sea from "node:sea";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { VERSION } from "./generated/version.js";
import { appCacheDir } from "./paths.js";

const ASSETS = [
  "qqnt-export-core.exe",
  "sqlcipher.exe",
  "libcrypto-3-x64.dll",
  "zlib1.dll",
];

let cachedRuntime = null;

export function materializeBundledRuntime() {
  if (cachedRuntime) {
    return cachedRuntime;
  }
  if (!sea.isSea()) {
    return null;
  }

  const directory = join(appCacheDir(), "runtime", VERSION);
  const core = join(directory, "qqnt-export-core.exe");
  mkdirSync(directory, { recursive: true });
  for (const name of ASSETS) {
    const destination = join(directory, name);
    let data;
    try {
      data = Buffer.from(sea.getAsset(name));
    } catch (error) {
      throw new Error(`发行包缺少资源 ${name}: ${error.message}`);
    }
    if (
      existsSync(destination) &&
      statSync(destination).size === data.length &&
      readFileSync(destination).equals(data)
    ) {
      continue;
    }
    const temporary = `${destination}.tmp`;
    rmSync(temporary, { force: true });
    writeFileSync(temporary, data);
    renameSync(temporary, destination);
  }

  cachedRuntime = { directory, core };
  return cachedRuntime;
}

export function bundledCorePath() {
  return materializeBundledRuntime()?.core || null;
}

export function bundledCoreDirectory() {
  return materializeBundledRuntime()?.directory || null;
}
