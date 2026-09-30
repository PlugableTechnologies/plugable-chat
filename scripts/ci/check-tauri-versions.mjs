// Fail fast when the tauri Rust crate and the @tauri-apps npm packages are on different
// major.minor versions. `tauri build` refuses to run in that state, but only at the very end
// of a ~20 minute Windows build; this catches it in seconds.
//   node scripts/ci/check-tauri-versions.mjs
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

/** Version of the `tauri` crate in a Cargo.lock text (LF or CRLF: Windows checkouts use CRLF). */
export function tauriCrateVersion(lockText) {
  return /\[\[package\]\]\r?\nname = "tauri"\r?\nversion = "([^"]+)"/.exec(lockText)?.[1];
}

export const minor = (v) => v.split('.').slice(0, 2).join('.');

function main() {
  const crate = tauriCrateVersion(readFileSync('Cargo.lock', 'utf8'));
  if (!crate) {
    console.error('tauri crate not found in Cargo.lock');
    process.exit(1);
  }
  let failed = false;
  for (const pkg of ['@tauri-apps/api', '@tauri-apps/cli']) {
    const installed = JSON.parse(readFileSync(`node_modules/${pkg}/package.json`, 'utf8')).version;
    const ok = minor(installed) === minor(crate);
    console.log(`${ok ? 'OK  ' : 'FAIL'} ${pkg} ${installed} vs tauri crate ${crate}`);
    if (!ok) failed = true;
  }
  if (failed) {
    console.error('Update the npm packages and the tauri crate together (same major.minor).');
    process.exit(1);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
