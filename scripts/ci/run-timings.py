#!/usr/bin/env python3
"""Per-job and per-step durations (steps of 2+ minutes) for recent ci.yml runs.
Usage: python3 scripts/ci/run-timings.py
A still-running job shows elapsed time so far."""
import json,subprocess,sys
from datetime import datetime, timezone
def t(s):
    d = datetime.fromisoformat(s.replace('Z','+00:00')) if s else None
    return d if d and d.year > 2000 else None   # GitHub reports 0001-01-01 for unfinished
repo="PlugableTechnologies/plugable-chat"
runs=json.loads(subprocess.check_output(["gh","run","list","--repo",repo,"--workflow","ci.yml","--limit","12","--json","databaseId,headSha,conclusion,status,createdAt"]))
for r in reversed(runs):
    jobs=json.loads(subprocess.check_output(["gh","run","view",str(r["databaseId"]),"--repo",repo,"--json","jobs"]))["jobs"]
    print(f'run {r["databaseId"]} {r["headSha"][:7]} {r["status"]}/{r["conclusion"]}')
    for j in jobs:
        a,b=t(j.get("startedAt")),t(j.get("completedAt"))
        if not a: continue
        dur=((b or datetime.now(timezone.utc))-a).total_seconds()/60
        print(f'   {j["name"]:38} {dur:5.1f} min {j.get("conclusion") or "running"}')
        for s in j["steps"]:
            sa,sb=t(s.get("startedAt")),t(s.get("completedAt"))
            if sa and sb and (sb-sa).total_seconds()>=120:
                print(f'        {s["name"][:60]:60} {(sb-sa).total_seconds()/60:5.1f} min')

