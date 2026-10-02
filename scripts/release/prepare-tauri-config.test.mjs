// `node --test scripts/release/prepare-tauri-config.test.mjs`

import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { prepareTauriConfig } from './prepare-tauri-config.mjs';

const script = path.join(path.dirname(fileURLToPath(import.meta.url)), 'prepare-tauri-config.mjs');

function runCli(env, out) {
  const clean = { ...process.env };
  delete clean.WITH_UPDATER;
  delete clean.KEYPAIR_ALIAS;
  return execFileSync(process.execPath, [script, out], {
    encoding: 'utf8',
    env: { ...clean, ...env },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
}

test('nothing set yields an empty override', () => {
  assert.deepEqual(prepareTauriConfig({}), {});
});

test('WITH_UPDATER=true turns on updater artifacts', () => {
  assert.deepEqual(prepareTauriConfig({ WITH_UPDATER: 'true' }), {
    bundle: { createUpdaterArtifacts: true },
  });
});

test('WITH_UPDATER other than "true" is ignored', () => {
  for (const value of ['false', '1', 'TRUE', '']) {
    assert.deepEqual(prepareTauriConfig({ WITH_UPDATER: value }), {}, value);
  }
});

test('KEYPAIR_ALIAS sets the smctl sign command', () => {
  assert.deepEqual(prepareTauriConfig({ KEYPAIR_ALIAS: 'key_123.prod-1' }), {
    bundle: {
      windows: { signCommand: 'smctl.exe sign --keypair-alias=key_123.prod-1 --input %1' },
    },
  });
});

test('both together', () => {
  assert.deepEqual(prepareTauriConfig({ WITH_UPDATER: 'true', KEYPAIR_ALIAS: 'k' }), {
    bundle: {
      createUpdaterArtifacts: true,
      windows: { signCommand: 'smctl.exe sign --keypair-alias=k --input %1' },
    },
  });
});

test('an empty KEYPAIR_ALIAS is the same as unset', () => {
  assert.deepEqual(prepareTauriConfig({ KEYPAIR_ALIAS: '' }), {});
});

test('a KEYPAIR_ALIAS that could break out of the command is refused', () => {
  for (const alias of ['a b', 'a;calc', 'a"b', 'a&b', '--input=x %1']) {
    assert.throws(() => prepareTauriConfig({ KEYPAIR_ALIAS: alias }), /KEYPAIR_ALIAS/, alias);
  }
});

test('the CLI writes the JSON file tauri build -c reads', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'oc-tauri-config-'));
  const out = path.join(dir, 'nested', 'overrides.json');
  runCli({ WITH_UPDATER: 'true' }, out);
  assert.deepEqual(JSON.parse(fs.readFileSync(out, 'utf8')), {
    bundle: { createUpdaterArtifacts: true },
  });

  runCli({}, out);
  assert.deepEqual(JSON.parse(fs.readFileSync(out, 'utf8')), {});
});

test('the CLI does not print the alias', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'oc-tauri-config-'));
  const stdout = runCli({ KEYPAIR_ALIAS: 'secret-alias' }, path.join(dir, 'o.json'));
  assert.doesNotMatch(stdout, /secret-alias/);
});

test('the CLI fails without an output path and on a bad alias', () => {
  assert.throws(() =>
    execFileSync(process.execPath, [script], { stdio: ['ignore', 'pipe', 'pipe'] }),
  );
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'oc-tauri-config-'));
  assert.throws(() => runCli({ KEYPAIR_ALIAS: 'bad alias' }, path.join(dir, 'o.json')));
});
