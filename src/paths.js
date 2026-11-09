import { homedir, platform } from "node:os";
import { join } from "node:path";

export function appConfigDir() {
  if (platform() === "win32") {
    return join(process.env.APPDATA || join(homedir(), "AppData", "Roaming"), "qqnt-export");
  }
  return join(process.env.XDG_CONFIG_HOME || join(homedir(), ".config"), "qqnt-export");
}

export function appCacheDir() {
  if (platform() === "win32") {
    return join(process.env.LOCALAPPDATA || join(homedir(), "AppData", "Local"), "qqnt-export");
  }
  return join(process.env.XDG_CACHE_HOME || join(homedir(), ".cache"), "qqnt-export");
}

export function configPath() {
  return join(appConfigDir(), "config.json");
}

export function accountCacheDir(account) {
  return join(appCacheDir(), "accounts", sanitizeSegment(account));
}

function sanitizeSegment(value) {
  const text = String(value || "custom").replace(/[^a-zA-Z0-9._-]+/g, "-");
  return text || "custom";
}
