import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { join } from "node:path";

for (const flag of ["-h", "--help", "help"]) {
  test(`CLI ${flag} shows help without the Rust core`, () => {
    const result = spawnSync(process.execPath, [join("bin", "qqnt-export.js"), flag], {
      encoding: "utf8",
    });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /qqnt-export/i);
    assert.match(result.stdout, /open/);
    assert.match(result.stdout, /auth/);
  });
}

test("subcommand help shows command options", () => {
  const result = spawnSync(
    process.execPath,
    [join("bin", "qqnt-export.js"), "open", "--help"],
    { encoding: "utf8" },
  );
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /--account/);
  assert.doesNotMatch(result.stdout, /--extract-key/);
});

test("auth help exposes the first-stage key capture command", () => {
  const result = spawnSync(
    process.execPath,
    [join("bin", "qqnt-export.js"), "auth", "-h"],
    { encoding: "utf8" },
  );
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /--account/);
  assert.match(result.stdout, /密钥/);
});

test("unknown options fail clearly", () => {
  const result = spawnSync(
    process.execPath,
    [join("bin", "qqnt-export.js"), "chats", "--does-not-exist"],
    { encoding: "utf8" },
  );
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /未知选项/);
});
