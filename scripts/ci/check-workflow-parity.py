#!/usr/bin/env python3
"""Fail when release.yml's build environment drifts from what CI proved works.

The first release-candidate Linux build failed because release.yml lacked two apt packages
(libprotobuf-dev, libgtk-3-dev) that ci.yml had needed since its first green run. Checks:
  - every apt package CI installs is also installed by release.yml (release may add more, e.g. rpm)
  - the pinned RUSTUP_TOOLCHAIN is the same
  - the pinned protoc download (version and sha256) is the same

  python3 scripts/ci/check-workflow-parity.py [ci.yml] [release.yml]
"""
import re
import sys

def read(p):
    return open(p, encoding="utf-8").read()

def apt_packages(text):
    pkgs = set()
    for m in re.finditer(r"apt-get install(?:[^\n]*\\\n)*[^\n]*", text):
        for tok in re.split(r"[\s\\]+", m.group(0)):
            if tok and not tok.startswith("-") and tok not in ("apt-get", "install", "sudo"):
                pkgs.add(tok)
    return pkgs

ci_path = sys.argv[1] if len(sys.argv) > 1 else ".github/workflows/ci.yml"
rel_path = sys.argv[2] if len(sys.argv) > 2 else ".github/workflows/release.yml"
ci, rel = read(ci_path), read(rel_path)
problems = []

if not apt_packages(ci) or not apt_packages(rel):
    problems.append("could not find the apt-get install lists (did the workflow layout change?)")

missing = apt_packages(ci) - apt_packages(rel)
if missing:
    problems.append(f"release.yml does not install apt packages that ci.yml does: {sorted(missing)}")

tc = lambda t: re.findall(r'RUSTUP_TOOLCHAIN:\s*"?([\d.]+)"?', t)[:1]
if tc(ci) != tc(rel):
    problems.append(f"Rust toolchain pin differs: ci {tc(ci)} vs release {tc(rel)}")

protoc = lambda t: (re.findall(r"protoc-([\d.]+)-win64", t)[:1], re.findall(r'-ne "([0-9a-f]{64})"', t)[:1])
if protoc(ci) != protoc(rel):
    problems.append(f"protoc pin differs: ci {protoc(ci)} vs release {protoc(rel)}")

if problems:
    print("\n".join(problems))
    sys.exit(1)
print("release.yml matches ci.yml: apt packages, Rust pin, protoc pin")
