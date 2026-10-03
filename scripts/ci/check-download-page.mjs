#!/usr/bin/env node
// Keeps the public download page honest: the README and release notes must name the files the release
// actually produces, and the user guide must stay UI-only. Run from the repo root.
import { readFileSync, existsSync } from 'node:fs';

export function problemsFor({ readme, guide, notes, releaseYml, tauriConf }) {
  const problems = [];
  const productName = tauriConf.productName;
  if (!productName) problems.push('tauri.conf.json has no productName');

  // Tauri names: <productName>_<version>_x64-setup.exe, <productName>_<version>_x64_en-US.msi, ..._amd64.AppImage
  const expected = [
    `${productName}_<version>_x64-setup.exe`,
    `${productName}_<version>_x64_en-US.msi`,
    `${productName}_<version>_amd64.AppImage`,
  ];
  for (const name of expected) {
    for (const [label, text] of [['README.md', readme], ['release notes template', notes]]) {
      if (!text.includes(name)) problems.push(`${label} does not mention ${name}`);
    }
  }
  if (!releaseYml.includes('release-notes-template.md')) {
    problems.push('release.yml does not use .github/release-notes-template.md');
  }
  if (!releaseYml.includes('SHA256SUMS-windows.txt')) {
    problems.push('release.yml does not write SHA256SUMS-windows.txt, which the release notes mention');
  }
  for (const [label, text] of [['README.md', readme], ['docs/user-guide.md', guide]]) {
    for (const m of text.matchAll(/(^|\s)(--[a-z][a-z-]+)/g)) {
      // Command-line flags belong in developer docs and the collapsed advanced block only.
      if (label === 'docs/user-guide.md') problems.push(`${label} contains a command-line flag: ${m[2]}`);
    }
  }
  const imgs = [...guide.matchAll(/!\[[^\]]*\]\((images\/[^)]+)\)/g)].map((m) => m[1]);
  for (const img of imgs) if (!existsSync(`docs/${img}`)) problems.push(`docs/user-guide.md links a missing image: ${img}`);
  return problems;
}

if (process.argv[1] && import.meta.url === new URL(`file://${process.argv[1]}`).href) {
  const read = (p) => readFileSync(p, 'utf8');
  const problems = problemsFor({
    readme: read('README.md'),
    guide: read('docs/user-guide.md'),
    notes: read('.github/release-notes-template.md'),
    releaseYml: read('.github/workflows/release.yml'),
    tauriConf: JSON.parse(read('src-tauri/tauri.conf.json')),
  });
  if (problems.length) { console.error(problems.map((p) => `- ${p}`).join('\n')); process.exit(1); }
  console.log('download page matches the release files');
}
