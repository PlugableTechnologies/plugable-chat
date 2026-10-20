import test from 'node:test';
import assert from 'node:assert/strict';
import { labelFromTag, renamedFor, problemsFor } from './release-names.mjs';

// R13 (Pat): rc10 and rc11 shipped as the same filename with different sha256.
test('rc10 and rc11 get different filenames', () => {
  const a = renamedFor('plugable-chat_0.1.0_x64-setup.exe', 'v0.1.0-rc10');
  const b = renamedFor('plugable-chat_0.1.0_x64-setup.exe', 'v0.1.0-rc11');
  assert.equal(a, 'plugable-chat_0.1.0-rc10_x64-setup.exe');
  assert.equal(b, 'plugable-chat_0.1.0-rc11_x64-setup.exe');
  assert.notEqual(a, b);
});

test('MSI names are renamed too, and final releases keep a clean name', () => {
  assert.equal(renamedFor('plugable-chat_0.1.0_x64_en-US.msi', 'v0.1.0-rc12'), 'plugable-chat_0.1.0-rc12_x64_en-US.msi');
  assert.equal(renamedFor('plugable-chat_0.1.0_x64-setup.exe', 'v0.1.0'), 'plugable-chat_0.1.0_x64-setup.exe');
});

test('a tag that is not a release tag is refused', () => {
  for (const bad of ['main', 'refs/heads/x', 'v1', 'v0.1.0-rc1/../x', '', undefined]) {
    assert.throws(() => labelFromTag(bad), /not a release tag/);
  }
});

test('an installer built for another version is refused', () => {
  assert.throws(() => renamedFor('plugable-chat_0.0.0_x64-setup.exe', 'v0.1.0-rc12'), /carries version 0.0.0/);
});

test('same tag + same name + different hash fails the release', () => {
  const files = [{ name: 'plugable-chat_0.1.0-rc11_x64-setup.exe', sha256: 'b' }];
  assert.deepEqual(problemsFor({ tag: 'v0.1.0-rc11', files, existing: [{ name: files[0].name, sha256: 'b' }] }), []);
  const p = problemsFor({ tag: 'v0.1.0-rc11', files, existing: [{ name: files[0].name, sha256: 'a' }] });
  assert.match(p[0], /different sha256/);
});

test('an unrenamed file fails the name check', () => {
  const p = problemsFor({ tag: 'v0.1.0-rc11', files: [{ name: 'plugable-chat_0.1.0_x64-setup.exe', sha256: 'a' }] });
  assert.match(p[0], /does not carry 0.1.0-rc11/);
});

test('macOS and Linux files carry the release too', () => {
  assert.equal(renamedFor('plugable-chat_0.1.0_aarch64.dmg', 'v0.1.0-rc14'), 'plugable-chat_0.1.0-rc14_aarch64.dmg');
  assert.equal(renamedFor('plugable-chat_0.1.0_amd64.deb', 'v0.1.0-rc14'), 'plugable-chat_0.1.0-rc14_amd64.deb');
  assert.equal(renamedFor('plugable-chat_0.1.0_amd64.AppImage', 'v0.1.0-rc14'), 'plugable-chat_0.1.0-rc14_amd64.AppImage');
  assert.equal(renamedFor('plugable-chat-0.1.0-1.x86_64.rpm', 'v0.1.0-rc14'), 'plugable-chat-0.1.0-rc14-1.x86_64.rpm');
});

test('a stable tag keeps the plain version in every platform name', () => {
  assert.equal(renamedFor('plugable-chat_0.1.0_aarch64.dmg', 'v0.1.0'), 'plugable-chat_0.1.0_aarch64.dmg');
});

test('problemsFor accepts the rpm form and rejects an unrenamed dmg', () => {
  assert.deepEqual(problemsFor({ tag: 'v0.1.0-rc14', files: [{ name: 'plugable-chat-0.1.0-rc14-1.x86_64.rpm', sha256: 'a' }] }), []);
  const p = problemsFor({ tag: 'v0.1.0-rc14', files: [{ name: 'plugable-chat_0.1.0_aarch64.dmg', sha256: 'a' }] });
  assert.match(p[0], /does not carry 0.1.0-rc14/);
});

test('a macOS or Linux file for another version is refused', () => {
  assert.throws(() => renamedFor('plugable-chat_0.0.0_aarch64.dmg', 'v0.1.0-rc14'), /carries version 0.0.0/);
});
