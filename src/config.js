import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname } from "node:path";
import { configPath } from "./paths.js";

const EMPTY_CONFIG = {
  version: 1,
  currentAccount: null,
  accounts: {},
};

export async function readConfig() {
  try {
    const parsed = JSON.parse(await readFile(configPath(), "utf8"));
    return {
      ...EMPTY_CONFIG,
      ...parsed,
      accounts: parsed.accounts && typeof parsed.accounts === "object" ? parsed.accounts : {},
    };
  } catch (error) {
    if (error?.code !== "ENOENT") {
      throw new Error(`无法读取配置: ${error.message}`);
    }
    return structuredClone(EMPTY_CONFIG);
  }
}

export async function writeConfig(config) {
  const path = configPath();
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, `${JSON.stringify(config, null, 2)}\n`, {
    encoding: "utf8",
    mode: 0o600,
  });
}

export async function rememberAccount(account, details) {
  const config = await readConfig();
  config.currentAccount = account;
  config.accounts[account] = {
    ...(config.accounts[account] || {}),
    ...details,
    updatedAt: new Date().toISOString(),
  };
  await writeConfig(config);
  return config;
}

export async function currentLibrary() {
  const config = await readConfig();
  const account = config.currentAccount;
  const record = account ? config.accounts[account] : null;
  return record?.library || null;
}
