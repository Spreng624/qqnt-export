import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { corePath, runCore } from "../src/core.js";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const sqlcipher = join(root, "vendor", "sqlcipher", "sqlcipher.exe");
const key = "0123456789abcdef";

if (process.platform === "win32") {
  process.env.QQNT_EXPORT_CORE = join(root, "core", "target", "debug", "qqnt-export-core.exe");
}

test(
  "decrypt, index, search and export through the CLI core",
  { skip: process.platform !== "win32" },
  async () => {
    corePath();
    const directory = await mkdtemp(join(tmpdir(), "qqnt-export-e2e-"));
    try {
      const encrypted = join(directory, "encrypted.db");
      const raw = join(directory, "nt_msg.db");
      const plain = join(directory, "plain.db");
      const library = join(directory, "library.db");
      const exported = join(directory, "chat.md");
      const bodyHex = messageBody("共同作业 fixture").toString("hex");
      const result = spawnSync(
        sqlcipher,
        [encrypted],
        {
          encoding: "utf8",
          input: `PRAGMA cipher_page_size = 4096;
PRAGMA key = '${key}';
PRAGMA kdf_iter = 4000;
PRAGMA cipher_hmac_algorithm = HMAC_SHA1;
PRAGMA cipher_kdf_algorithm = PBKDF2_HMAC_SHA512;
CREATE TABLE group_msg_table(
  "40001" INTEGER PRIMARY KEY, "40050" INTEGER, "40013" INTEGER,
  "40020" TEXT, "40033" INTEGER, "40021" TEXT, "40030" INTEGER,
  "40011" INTEGER, "40012" INTEGER, "40090" TEXT, "40093" TEXT,
  "40800" BLOB
);
INSERT INTO group_msg_table VALUES(
  1, 1760000000, 0, 'u_peer', 10001, '20002', 20002, 2, 0,
  'Peer', 'Peer', X'${bodyHex}'
);`,
        },
      );
      assert.equal(result.status, 0, result.stderr);

      const prefix = Buffer.alloc(1024);
      prefix.fill("a", 0, 128);
      await writeFile(raw, Buffer.concat([prefix, await readFile(encrypted)]));

      const decrypted = await runCore(
        ["decrypt", raw, plain],
        { stdinText: `${key}\n`, quiet: true },
      );
      assert.equal(decrypted.headerSize, 1024);

      const indexed = await runCore(
        ["index", plain, library, "--replace"],
        { quiet: true },
      );
      assert.equal(indexed.messages, 1);

      const found = await runCore(
        ["search", library, "作业", "--limit", "10"],
        { quiet: true },
      );
      assert.equal(found.count, 1);
      assert.equal(found.messages[0].text, "共同作业 fixture");

      const exportedResult = await runCore(
        ["export", library, "group:20002", exported, "--format", "markdown"],
        { quiet: true },
      );
      assert.equal(exportedResult.messageCount, 1);
      assert.match(await readFile(exported, "utf8"), /共同作业 fixture/);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  },
);

function messageBody(text) {
  const segment = Buffer.concat([
    field(45002, varint(1)),
    field(45101, Buffer.from(text)),
  ]);
  return field(40800, segment);
}

function field(number, payload) {
  return Buffer.concat([varint((BigInt(number) << 3n) | 2n), varint(payload.length), payload]);
}

function varint(value) {
  let number = BigInt(value);
  const bytes = [];
  while (number >= 0x80n) {
    bytes.push(Number(number & 0x7fn) | 0x80);
    number >>= 7n;
  }
  bytes.push(Number(number));
  return Buffer.from(bytes);
}
