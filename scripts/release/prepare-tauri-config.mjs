#!/usr/bin/env node
// Build-time overrides merged over `crates/opencompany-app/tauri.conf.json` by
// `tauri build -c <path>`. The static file stays the source of truth for the
// updater pubkey and endpoint; only values that exist at build time live here.
//
//   WITH_UPDATER=true  → bundle.createUpdaterArtifacts, so the bundler signs
//                        the installers it emits. Set on the Windows leg only:
//                        the macOS updater archive is built after notarization
//                        by package-updater-artifact.sh.
//   KEYPAIR_ALIAS=<a>  → bundle.windows.signCommand through DigiCert smctl.
//
// Neither set → `{}`.
//
//   node scripts/release/prepare-tauri-config.mjs <output.json>

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const KEYPAIR_ALIAS = /^[A-Za-z0-9._-]+$/;

/** The override object for the given environment. */
export function prepareTauriConfig(env = process.env) {
  const bundle = {};

  if (env.WITH_UPDATER === 'true') {
    bundle.createUpdaterArtifacts = true;
  }

  const alias = env.KEYPAIR_ALIAS ?? '';
  if (alias !== '') {
    if (!KEYPAIR_ALIAS.test(alias)) {
      throw new Error('KEYPAIR_ALIAS must be letters, digits, ".", "_" or "-" only');
    }
    bundle.windows = {
      signCommand: `smctl.exe sign --keypair-alias=${alias} --input %1`,
    };
  }

  return Object.keys(bundle).length > 0 ? { bundle } : {};
}

function main(argv) {
  const out = argv[2];
  if (!out) {
    console.error('usage: prepare-tauri-config.mjs <output.json>');
    return 2;
  }
  const config = prepareTauriConfig();
  fs.mkdirSync(path.dirname(path.resolve(out)), { recursive: true });
  fs.writeFileSync(out, `${JSON.stringify(config)}\n`);
  console.log(`[tauri-config] overrides → ${out}: ${JSON.stringify(Object.keys(config.bundle ?? {}))}`);
  return 0;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    process.exitCode = main(process.argv);
  } catch (err) {
    console.error(`[tauri-config] ${err.message}`);
    process.exitCode = 1;
  }
}
