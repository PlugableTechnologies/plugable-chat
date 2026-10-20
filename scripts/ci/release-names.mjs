#!/usr/bin/env node
// Names the release files (Windows, macOS, Linux) after the tag. Tauri names them from the numeric version, and the
// MSI version cannot carry "-rcN", so rc10 and rc11 both came out as plugable-chat_0.1.0_x64-setup.exe
// with different contents. Renaming after Authenticode signing is safe: that signature does not cover the
// filename. Sigstore signatures and checksums do, so macOS and Linux files are renamed before those.
//
//   node scripts/ci/release-names.mjs <tag> <dir>   rename every installer in <dir> in place
//
// Tauri's names: plugable-chat_0.1.0_x64-setup.exe, ..._x64_en-US.msi, ..._aarch64.dmg, ..._amd64.deb,
// ..._amd64.AppImage, and plugable-chat-0.1.0-1.x86_64.rpm (the rpm uses '-' where the others use '_').
import { readdirSync, renameSync } from 'node:fs';
import { join } from 'node:path';

// "v0.1.0-rc12" -> "0.1.0-rc12". Anything that is not a plain release tag is refused, so a stray
// ref name can never leak into a filename.
export function labelFromTag(tag) {
  const m = /^v(\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)$/.exec(tag ?? '');
  if (!m) throw new Error(`not a release tag: ${JSON.stringify(tag)}`);
  return m[1];
}

// plugable-chat_0.1.0_x64-setup.exe + v0.1.0-rc12 -> plugable-chat_0.1.0-rc12_x64-setup.exe
export function renamedFor(file, tag) {
  const label = labelFromTag(tag);
  const numeric = label.split('-')[0];
  const m = /^(.+?)([_-])(\d+\.\d+\.\d+)([_-].+\.(?:exe|msi|dmg|deb|rpm|AppImage))$/.exec(file);
  if (!m) throw new Error(`unrecognised installer name: ${file}`);
  if (m[3] !== numeric) throw new Error(`${file} carries version ${m[3]} but the tag is ${tag}`);
  return `${m[1]}${m[2]}${label}${m[4]}`;
}

// A tag that already has a file of the same name with a different hash means someone re-ran or
// re-pushed a release: refuse rather than let two different builds share one name.
export function problemsFor({ tag, files, existing = [] }) {
  const problems = [];
  const label = labelFromTag(tag);
  for (const f of files) {
    if (!f.name.includes(`_${label}_`) && !f.name.includes(`-${label}-`)) problems.push(`${f.name} does not carry ${label}`);
    const prior = existing.find((e) => e.name === f.name);
    if (prior && prior.sha256 !== f.sha256) {
      problems.push(`${f.name} already exists for ${tag} with a different sha256`);
    }
  }
  return problems;
}

if (process.argv[1] && import.meta.url === new URL(`file://${process.argv[1]}`).href) {
  const [tag, dir] = process.argv.slice(2);
  if (!tag || !dir) { console.error('usage: release-names.mjs <tag> <dir>'); process.exit(2); }
  for (const f of readdirSync(dir).filter((n) => /\.(exe|msi|dmg|deb|rpm|AppImage)$/.test(n))) {
    const to = renamedFor(f, tag);
    renameSync(join(dir, f), join(dir, to));
    console.log(`${f} -> ${to}`);
  }
}
