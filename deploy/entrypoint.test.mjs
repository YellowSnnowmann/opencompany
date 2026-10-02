import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const entrypoint = path.join(root, "deploy", "entrypoint.sh");

function run(company) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "opencompany-entrypoint-"));
  const bin = path.join(dir, "bin");
  fs.mkdirSync(bin);
  fs.writeFileSync(
    path.join(bin, "opencompany"),
    '#!/bin/sh\nprintf "%s\\n" "$@" > "$CAPTURE_ARGS"\n',
    { mode: 0o755 },
  );
  if (company) {
    const companyDir = path.join(dir, "companies", company);
    fs.mkdirSync(companyDir, { recursive: true });
    fs.writeFileSync(path.join(companyDir, "company.toml"), "[company]\nname = \"Test\"\n");
  }
  const capture = path.join(dir, "args.txt");
  const env = {
    ...process.env,
    PATH: `${bin}${path.delimiter}${process.env.PATH}`,
    CAPTURE_ARGS: capture,
    OPENCOMPANY_COMPANY: company ?? "",
    OPENCOMPANY_BIND: "0.0.0.0:8080",
    OPENCOMPANY_DATA_DIR: "/data",
  };
  const result = spawnSync("sh", [entrypoint], { cwd: dir, env, encoding: "utf8" });
  const args = fs.existsSync(capture) ? fs.readFileSync(capture, "utf8").trimEnd().split("\n") : [];
  fs.rmSync(dir, { recursive: true, force: true });
  return { ...result, args };
}

test("an explicitly blank company starts the host without a company bundle", () => {
  const result = run("");
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.args, ["serve", "--bind", "0.0.0.0:8080", "--home", "/data"]);
});

test("a named company still selects its bundled manifest", () => {
  const result = run("marketing_agency");
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.args, [
    "serve",
    "--company",
    "companies/marketing_agency",
    "--bind",
    "0.0.0.0:8080",
    "--home",
    "/data",
  ]);
});
