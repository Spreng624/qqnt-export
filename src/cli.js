import { existsSync } from "node:fs";
import { mkdir, rm, stat } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { join, resolve } from "node:path";
import { currentLibrary, readConfig, rememberAccount, writeConfig } from "./config.js";
import { runCore } from "./core.js";
import { VERSION } from "./generated/version.js";
import { accountCacheDir, appCacheDir } from "./paths.js";
import { printAccounts, printChats, printDoctor, printMessages } from "./render.js";
import {
  ask,
  confirm,
  isInteractive,
} from "./terminal.js";

const VALUE_OPTIONS = new Set([
  "account",
  "after",
  "before",
  "chat",
  "data-root",
  "format",
  "library",
  "limit",
  "output",
]);

const BOOLEAN_OPTIONS = new Set([
  "help",
  "json",
  "version",
  "yes",
]);

const SHORT_OPTIONS = {
  a: "account",
  f: "format",
  h: "help",
  j: "json",
  n: "limit",
  o: "output",
  V: "version",
  y: "yes",
};

export async function main(argv) {
  await dispatch(argv);
}

async function dispatch(argv) {
  const parsed = parseArgs(argv);

  if (parsed.options.version) {
    console.log(`qqnt-export ${VERSION}`);
    return;
  }

  const command = parsed.command?.toLowerCase();
  if (parsed.options.help || !command || command === "help") {
    printHelp(command === "help" ? parsed.positionals[0] : command);
    return;
  }

  switch (command) {
    case "doctor":
      return runDoctor(parsed.options);
    case "auth":
      return runAuth(parsed.options);
    case "open":
      return runOpen(parsed.options);
    case "chats":
      return runChats(parsed.options);
    case "history":
      return runHistory(parsed.positionals[0], parsed.options);
    case "search":
      return runSearch(parsed.positionals[0], parsed.options);
    case "export":
      return runExport(parsed.positionals[0], parsed.options);
    case "clean":
      return runClean(parsed.options);
    default:
      throw new Error(`未知命令 ${command}。运行 qqnt-export -h 查看帮助。`);
  }
}

async function runDoctor(options) {
  const args = ["doctor"];
  if (options["data-root"]) {
    args.push("--data-root", resolve(options["data-root"]));
  }
  const result = await runCore(args);
  if (options.json) {
    printJson(result);
  } else {
    printDoctor(result);
  }
}

async function runOpen(options) {
  const selected = await selectAccount(await runCore(["accounts"]), options);

  await confirmDataProcessing();
  const cacheDir = accountCacheDir(selected.account);
  await mkdir(cacheDir, { recursive: true });
  const plainPath = join(cacheDir, "nt_msg_plain.db");
  const library = join(cacheDir, "library.db");

  console.log(`正在解密账号 ${selected.account}...`);
  const decrypted = await runCore(
    [
      "decrypt",
      selected.dbPath,
      plainPath,
      "--force",
      "--account-key",
      selected.account,
    ],
  );

  try {
    console.log("正在建立消息索引...");
    const indexed = await runCore(["index", decrypted.output, library, "--replace"]);
    await rememberAccount(selected.account, {
      sourceDb: selected.dbPath,
      library,
      lastIndexedAt: new Date().toISOString(),
    });

    console.log(`账号: ${selected.account}`);
    console.log(`索引: ${library}`);
    console.log(`完成: ${indexed.messages} 条消息，${indexed.chats} 个会话`);
  } finally {
    for (const suffix of ["", "-wal", "-shm"]) {
      await rm(`${plainPath}${suffix}`, { force: true });
    }
  }
}

async function runAuth(options) {
  const accountsResult = await runCore(["accounts"]);
  const selected = await selectAccount(accountsResult, options);
  if (isInteractive()) {
    const accepted = await confirm(
      `auth 将调试启动 QQ 并截获账号 ${selected.account} 的数据库密钥。继续？`,
      false,
    );
    if (!accepted) {
      throw new Error("已取消");
    }
  }

  if (qqIsRunning()) {
    if (!isInteractive()) {
      throw new Error("QQ 正在运行，请完全退出 QQ 后重新执行 auth");
    }
    console.log("检测到 QQ 正在运行。");
    console.log("请现在完全退出 QQ，包括托盘中的 QQ。");
    console.log("程序会在 QQ 退出后自动继续，最多等待 120 秒。");
    if (!(await waitForQQExit(120_000))) {
      throw new Error("等待 QQ 退出超时，请完全退出 QQ 后重新执行 auth");
    }
    console.log("QQ 已退出。");
  }

  console.log("正在启动调试模式 QQ。");
  console.log(`请在弹出的 QQ 窗口中登录或确认账号 ${selected.account}。`);
  console.log("密钥截获成功后，调试 QQ 会自动关闭。");

  const result = await runCore(["auth", selected.account, selected.dbPath]);
  console.log(`账号 ${result.account} 的密钥已安全保存。`);
  console.log(`QQ 版本: ${result.version || "未知"}`);
  console.log("现在可以运行 open 建立本地索引。");
}

function qqIsRunning() {
  const result = spawnSync(
    "tasklist",
    ["/FI", "IMAGENAME eq QQ.exe", "/FO", "CSV", "/NH"],
    { encoding: "utf8", windowsHide: true },
  );
  return result.status === 0 && result.stdout.includes('"QQ.exe"');
}

async function waitForQQExit(timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (!qqIsRunning()) {
      return true;
    }
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  return !qqIsRunning();
}

async function selectAccount(accountsResult, options) {
  let account = options.account ? String(options.account) : null;
  const accounts = accountsResult.accounts;
  if (!accounts.length) {
    throw new Error("未找到 QQ NT 数据，请先登录一次 QQ。");
  }
  if (!account && accounts.length === 1) {
    account = accounts[0].account;
  }
  if (!account && isInteractive()) {
    printAccounts(accountsResult);
    account = await ask("选择 QQ 账号");
  }
  if (!account) {
    throw new Error("检测到多个账号，请用 --account 指定");
  }
  const selected = accounts.find((item) => String(item.account) === account);
  if (!selected) {
    throw new Error(`未找到账号 ${account}`);
  }
  return selected;
}

async function runChats(options) {
  const library = await requireLibrary(options);
  const result = await runCore(["chats", library]);
  if (options.json) {
    printJson(result);
  } else {
    printChats(result.chats);
  }
}

async function runHistory(chat, options) {
  if (!chat) {
    throw new Error("用法: qqnt-export history <会话> [-n 50]");
  }
  const library = await requireLibrary(options);
  const args = [
    "history",
    library,
    chat,
    "--limit",
    String(parseLimit(options.limit, 50)),
  ];
  appendFilters(args, options);
  const result = await runCore(args);
  if (options.json) {
    printJson(result);
  } else {
    console.log(`会话 ${result.chat}: ${result.count} 条消息`);
    printMessages(result.messages);
  }
}

async function runSearch(query, options) {
  if (!query) {
    throw new Error('用法: qqnt-export search "关键词" [--chat 会话]');
  }
  const library = await requireLibrary(options);
  const args = [
    "search",
    library,
    query,
    "--limit",
    String(parseLimit(options.limit, 50)),
  ];
  if (options.chat) {
    args.push("--chat", options.chat);
  }
  appendFilters(args, options);
  const result = await runCore(args);
  if (options.json) {
    printJson(result);
  } else {
    console.log(`“${result.query}”找到 ${result.count} 条消息`);
    printMessages(result.messages);
  }
}

async function runExport(chat, options) {
  if (!chat) {
    throw new Error("用法: qqnt-export export <会话> [-f markdown] [-o 文件]");
  }
  const library = await requireLibrary(options);
  const format = String(options.format || "markdown").toLowerCase();
  const extension = {
    json: "json",
    jsonl: "jsonl",
    ndjson: "jsonl",
    csv: "csv",
    md: "md",
    markdown: "md",
    html: "html",
  }[format];
  if (!extension) {
    throw new Error(`不支持的格式 ${format}`);
  }

  const output = resolve(
    options.output || `${sanitizeFilename(chat)}-${dateStamp()}.${extension}`,
  );
  const args = ["export", library, chat, output, "--format", format];
  if (options.limit !== undefined) {
    args.push(
      "--limit",
      String(parseLimit(options.limit, 100_000, 10_000_000)),
    );
  }
  appendFilters(args, options);
  const result = await runCore(args);
  if (options.json) {
    printJson(result);
  } else {
    console.log(`已导出 ${result.messageCount} 条消息: ${result.output}`);
  }
}

async function runClean(options) {
  const target = appCacheDir();
  if (!existsSync(target)) {
    console.log("缓存为空。");
    return;
  }

  if (!options.yes && isInteractive()) {
    const accepted = await confirm(`删除本地缓存 ${target}？`, false);
    if (!accepted) {
      throw new Error("已取消");
    }
  } else if (!options.yes) {
    throw new Error("非交互环境清理缓存时必须加 --yes");
  }

  await rm(target, { recursive: true, force: true });
  const config = await readConfig();
  for (const account of Object.keys(config.accounts)) {
    delete config.accounts[account].library;
  }
  config.currentAccount = null;
  await writeConfig(config);
  console.log(`已清理: ${target}`);
}

async function requireLibrary(options) {
  const library = resolve(options.library || (await currentLibrary()) || "");
  if (!library || library === resolve("")) {
    throw new Error("尚未打开账号。先运行 qqnt-export open。");
  }
  try {
    await stat(library);
  } catch {
    throw new Error(`索引不存在: ${library}。请重新运行 qqnt-export open。`);
  }
  return library;
}

async function confirmDataProcessing() {
  if (!isInteractive()) {
    return;
  }
  const accepted = await confirm(
    "将在本机生成未加密聊天索引。确认仅处理本人或已获授权的数据？",
    false,
  );
  if (!accepted) {
    throw new Error("已取消");
  }
}

function appendFilters(args, options) {
  for (const name of ["before", "after"]) {
    if (options[name] !== undefined) {
      args.push(`--${name}`, String(parseTimestamp(options[name])));
    }
  }
}

function parseArgs(argv) {
  const positionals = [];
  const options = {};

  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index];
    if (token === "-" || !token.startsWith("-")) {
      positionals.push(token);
      continue;
    }

    let name;
    let inlineValue;
    if (token.startsWith("--")) {
      [name, inlineValue] = token.slice(2).split(/=(.*)/s, 2);
    } else {
      const short = token.slice(1);
      if (short.length !== 1 || !SHORT_OPTIONS[short]) {
        throw new Error(`未知选项 ${token}`);
      }
      name = SHORT_OPTIONS[short];
    }

    if (VALUE_OPTIONS.has(name)) {
      const value = inlineValue ?? argv[index + 1];
      if (value === undefined || value.startsWith("-")) {
        throw new Error(`选项 --${name} 缺少值`);
      }
      options[name] = value;
      if (inlineValue === undefined) {
        index += 1;
      }
    } else if (BOOLEAN_OPTIONS.has(name)) {
      options[name] = true;
    } else {
      throw new Error(`未知选项 --${name}`);
    }
  }

  return {
    command: positionals[0],
    positionals: positionals.slice(1),
    options,
  };
}

function parseTimestamp(value) {
  if (/^\d+$/.test(String(value))) {
    return Number(value);
  }
  const timestamp = Date.parse(String(value));
  if (!Number.isFinite(timestamp)) {
    throw new Error(`无法解析时间 ${value}`);
  }
  return Math.floor(timestamp / 1000);
}

function parseLimit(value, fallback, maximum = 100_000) {
  if (value === undefined) {
    return fallback;
  }
  const parsed = Number.parseInt(value, 10);
  if (!Number.isFinite(parsed) || parsed < 1 || parsed > maximum) {
    throw new Error(`limit 必须在 1 到 ${maximum} 之间`);
  }
  return parsed;
}

function sanitizeFilename(value) {
  return String(value).replace(/[<>:"/\\|?*\x00-\x1f]/g, "-").slice(0, 80) || "chat";
}

function dateStamp() {
  return new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
}

function printJson(value) {
  process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
}

function printHelp(topic) {
  const common = `Options:
  -h, --help      显示帮助
  -V, --version   显示版本`;

  const pages = {
    doctor: `Usage: qqnt-export doctor [options]

检查 QQ 数据、SQLCipher 和运行环境。

Options:
  --data-root <目录>  QQ 数据目录
  -j, --json          输出 JSON
  -h, --help          显示帮助`,
    auth: `Usage: qqnt-export auth [options]

提示退出当前 QQ，调试启动并截获数据库密钥后安全保存。

Options:
  -a, --account <QQ号>  选择账号
  -h, --help            显示帮助`,
    open: `Usage: qqnt-export open [options]

使用已保存的密钥读取账号并建立本地消息索引。

Options:
  -a, --account <QQ号>  选择账号
  -h, --help            显示帮助`,
    chats: `Usage: qqnt-export chats [options]

列出会话。

Options:
  --library <文件>  指定索引
  -j, --json        输出 JSON
  -h, --help        显示帮助`,
    history: `Usage: qqnt-export history <会话> [options]

读取会话消息。会话可写成 group:ID、c2c:ID 或纯号码。

Options:
  -n, --limit <数量>  返回数量，默认 50
  --library <文件>    指定索引
  --before <时间>     早于指定时间
  --after <时间>      晚于指定时间
  -j, --json          输出 JSON
  -h, --help          显示帮助`,
    search: `Usage: qqnt-export search <关键词> [options]

全文搜索消息。

Options:
  --chat <会话>       限制到指定会话
  -n, --limit <数量>  返回数量，默认 50
  --library <文件>    指定索引
  -j, --json          输出 JSON
  -h, --help          显示帮助`,
    export: `Usage: qqnt-export export <会话> [options]

导出会话。支持 markdown、html、json、jsonl、csv。

Options:
  -f, --format <格式>  默认 markdown
  -o, --output <文件>  输出文件
  -n, --limit <数量>   最多导出数量
  --library <文件>     指定索引
  -h, --help           显示帮助`,
    clean: `Usage: qqnt-export clean [options]

删除本地明文索引和运行缓存。

Options:
  -y, --yes   跳过确认
  -h, --help  显示帮助`,
  };

  if (topic && pages[topic]) {
    console.log(pages[topic]);
    return;
  }

  console.log(`qqnt-export ${VERSION}

读取、搜索和导出本机 QQNT 聊天记录。

Usage:
  qqnt-export <command> [options]

Commands:
  doctor    检查环境
  auth      获取并安全保存数据库密钥
  open      使用已保存密钥建立索引
  chats     列出会话
  history   查看会话消息
  search    搜索消息
  export    导出会话
  clean     清理本地索引

${common}

运行 qqnt-export <command> -h 查看命令选项。`);
}
