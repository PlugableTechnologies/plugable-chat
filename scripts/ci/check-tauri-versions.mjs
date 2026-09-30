// Fail fast when the tauri Rust crate and the @tauri-apps npm packages are on different
// major.minor versions. `tauri build` refuses to run in that state, but only at the very end
// of a ~20 minute Windows build; this catches it in seconds.
//   node scripts/ci/check-tauri-versions.mjs
import { readFileSync } from 'node:fs';

const lock = readFileSync('Cargo.lock', 'utf8');
const crate = /\[\[package\]\]\nname = "tauri"\nversion = "([^"]+)"/.exec(lock)?.[1];
if (!crate) {
  console.error('tauri crate not found in Cargo.lock');
  process.exit(1);
}
const minor = (v) => v.split('.').slice(0, 2).join('.');

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
