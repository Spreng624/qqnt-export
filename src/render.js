import { formatTime } from "./terminal.js";

export function printDoctor(result) {
  console.log("qqnt-export 环境检查");
  console.log(`平台: ${result.platform}`);
  console.log(`SQLCipher: ${result.sqlcipher ? "可用" : "不可用"}`);
  console.log(`QQ 数据目录: ${result.dataRoots.length ? "已找到" : "未找到"}`);
  for (const root of result.dataRoots) {
    console.log(`  ${root.path}${root.exists ? "" : " (不存在)"}`);
  }
  console.log(`QQ 账号: ${result.accounts.length}`);
  for (const account of result.accounts) {
    console.log(`  ${account.account}: ${account.dbPath}`);
  }
  if (result.warnings.length) {
    console.log("提示:");
    for (const warning of result.warnings) {
      console.log(`  - ${warning}`);
    }
  }
}

export function printAccounts(result) {
  if (!result.accounts.length) {
    console.log("未找到 QQ NT 账号数据库。");
    return;
  }
  for (const account of result.accounts) {
    const size = account.dbSize ? `${(account.dbSize / 1024 / 1024).toFixed(1)} MB` : "大小未知";
    console.log(`${account.account}  ${size}  ${account.dbPath}`);
  }
}

export function printChats(chats) {
  if (!chats.length) {
    console.log("没有会话。");
    return;
  }
  for (const chat of chats) {
    const kind = chat.kind === "group" ? "群" : "私聊";
    const last = formatTime(chat.lastTimestamp);
    console.log(`${chat.address.padEnd(18)} ${kind}  ${String(chat.messageCount).padStart(6)} 条  ${last}  ${chat.title}`);
  }
}

export function printMessages(messages) {
  if (!messages.length) {
    console.log("没有匹配的消息。");
    return;
  }
  for (const message of messages) {
    const sender = message.senderName || message.senderQq || message.senderUid || "未知发送者";
    const direction = message.direction === 1 || message.direction === 2 ? "我" : sender;
    const text = message.text || `[${message.contentType || "非文本消息"}]`;
    console.log(`[${formatTime(message.timestamp)}] ${direction}: ${text.replace(/\s+/g, " ")}`);
  }
}
