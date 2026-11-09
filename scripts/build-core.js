import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const debug = process.argv.includes("--debug");
const args = ["build", "--manifest-path", join(root, "core", "Cargo.toml")];

if (!debug) {
  args.push("--release");
}

const result = spawnSync("cargo", args, {
  cwd: root,
  stdio: "inherit",
  env: localBuildEnvironment(root),
});

if (result.error) {
  throw result.error;
}
if (result.status !== 0) {
  process.exit(result.status ?? 1);
}

const binary = process.platform === "win32"
  ? "qqnt-export-core.exe"
  : "qqnt-export-core";
const source = join(root, "core", "target", debug ? "debug" : "release", binary);
const destinationDir = join(root, "build", debug ? "runtime-debug" : "runtime");
const destination = join(destinationDir, binary);

mkdirSync(destinationDir, { recursive: true });
const sqlcipherSource = join(root, "vendor", "sqlcipher");
if (existsSync(sqlcipherSource)) {
  const sqlcipherDestination = join(destinationDir, "sqlcipher");
  rmSync(sqlcipherDestination, { recursive: true, force: true });
  cpSync(sqlcipherSource, sqlcipherDestination, { recursive: true });
}
cpSync(source, destination);

process.stdout.write(`Rust core: ${destination}\n`);

function localBuildEnvironment(root) {
  const environment = { ...process.env };
  if (process.platform === "win32") {
    const perlLib = join(root, ".toolchains", "perl-lib");
    if (existsSync(perlLib)) {
      environment.PERL5LIB = `${perlLib};${environment.PERL5LIB || ""}`;
    }
    const portablePerl = join(
      root,
      ".toolchains",
      "strawberry-perl",
      "perl",
      "bin",
      "perl.exe",
    );
    if (existsSync(portablePerl)) {
      environment.PATH = `${dirname(portablePerl)};${environment.PATH || ""}`;
    }
  }
  return environment;
}
