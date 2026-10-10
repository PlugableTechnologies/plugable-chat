#!/usr/bin/env bash
# Run the clean-host fault matrix on the GPU box and bring the JUnit results back.
#
#   ./run-matrix.sh <instance-id> <s3-bucket> <installer-url> [scenario,scenario|gpu|cpu|all]
#
# Environment:
#   OLD_INSTALLER_URL   older rc setup.exe for upgrade-over-old
#   VCREDIST_URL        vc_redist.x64.exe, lets no-vcredist remove the runtime first
#   EXPECT_FAIL=1       every scenario must be RED (run against rc9)
#   WAIT=3600           ceiling in seconds for the whole matrix
#   OUT=./matrix-out    where the results zip and JUnit land
#
# Installer URLs are presigned S3 (or release) links the box can fetch; the box has no IAM role in
# production. Scripts are fetched on the box by COMMIT HASH, so HEAD must be pushed. Needs the
# temporary SSM role (SPIKE_SSM=1 ./launch.sh), like ask.sh.
#
# Which host proves what (docs/clean-host-testing.md has the full table):
#   g5 box with the NVIDIA driver  gpu-baseline, baseline, quarantined-dll, upgrade-over-old
#   same box, driver rolled back   driver-absent
#   CPU box (bootstrap -SkipGpuDriver, INSTANCE_TYPE=m5.xlarge)  driver-absent, no-vcredist
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
IID="$1"; BUCKET="$2"; INSTALLER_URL="$3"; SCEN="${4:-gpu}"
OUT="${OUT:-$HERE/matrix-out}"; mkdir -p "$OUT"
WAIT="${WAIT:-3600}"
SHA="$(cd "$HERE" && git rev-parse HEAD)"

if [ -z "$(cd "$HERE" && git branch -r --contains "$SHA" 2>/dev/null)" ]; then
  echo "HEAD $SHA is not on origin; push it first (the box fetches the scripts by hash)." >&2
  exit 2
fi

KEY="matrix-$IID-$(date +%s).zip"
PUT="$(python3 -c "import boto3;print(boto3.client('s3').generate_presigned_url('put_object',Params={'Bucket':'$BUCKET','Key':'$KEY'},ExpiresIn=7200))")"

ARGS_B64="$(python3 - "$SHA" "$INSTALLER_URL" "$SCEN" "$PUT" <<'PY'
import base64, json, os, sys
sha, inst, scen, put = sys.argv[1:5]
d = {"Sha": sha, "Installer": inst, "Scenario": scen.split(","), "PutUrl": put,
     "OldInstaller": os.environ.get("OLD_INSTALLER_URL", ""), "VcRedist": os.environ.get("VCREDIST_URL", ""),
     "ExpectFail": os.environ.get("EXPECT_FAIL", "0") == "1"}
print(base64.b64encode(json.dumps(d).encode()).decode())
PY
)"

RAWBASE="https://raw.githubusercontent.com/PlugableTechnologies/plugable-chat/$SHA/infra/aws-gpu"
# The matrix runs in the desktop session (session 1) as Administrator, like a person using the box.
# Arguments travel as base64 JSON so no URL or scenario name ever meets shell quoting.
"$HERE/ssm.sh" "$IID" "
\$ProgressPreference='SilentlyContinue'
foreach (\$f in 'clean-host-matrix.ps1','run-in-session.ps1') { Invoke-WebRequest -UseBasicParsing -Uri ('$RAWBASE/' + \$f) -OutFile ('C:\\gpu\\' + \$f) }
[IO.File]::WriteAllBytes('C:\\gpu\\matrix-args.json', [Convert]::FromBase64String('$ARGS_B64'))
powershell -NoProfile -ExecutionPolicy Bypass -File C:\\gpu\\run-in-session.ps1 -Script C:\\gpu\\clean-host-matrix.ps1 -Args '-ArgsFile C:\\gpu\\matrix-args.json' -TimeoutSec $WAIT
Get-Content C:\\gpu\\matrix-transcript.txt -Tail 40" $((WAIT + 300)) || true

aws s3 cp "s3://$BUCKET/$KEY" "$OUT/clean-host-out.zip" --only-show-errors
unzip -o -q "$OUT/clean-host-out.zip" -d "$OUT/clean-host-out"
echo "results: $OUT/clean-host-out/clean-host-junit.xml"
python3 - "$OUT/clean-host-out/clean-host-junit.xml" <<'PY'
import sys, xml.etree.ElementTree as ET
s = ET.parse(sys.argv[1]).getroot().find("testsuite")
bad = 0
for t in s.findall("testcase"):
    f, k = t.find("failure"), t.find("skipped")
    state = "FAILED: " + f.get("message") if f is not None else ("skipped: " + k.get("message") if k is not None else "ok")
    bad += f is not None
    print(f"{t.get('name')}: {state}")
sys.exit(1 if bad else 0)
PY
