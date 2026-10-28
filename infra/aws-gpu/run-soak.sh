#!/usr/bin/env bash
# Run the 20-prompt soak (soak-run.ps1) on the GPU box and bring the result back.
#
#   ./run-soak.sh <instance-id> <s3-bucket> [model]
#
# Environment:
#   WAIT=9000     ceiling in seconds for the whole soak
#   OUT=./soak-out where soak-result.txt, the chat dumps and the reply files land
# The app must be installed at C:\Program Files\plugable-chat (soak-run.ps1 -AppDir overrides it on the box).
#
# Needs the installed app and C:\gpu\chicago-followups.json on the box (ask.sh puts the questions there), the
# temporary SSM role (SPIKE_SSM=1 ./launch.sh) and Node (bootstrap-box.ps1 installs it). Scripts are fetched on
# the box by COMMIT HASH, so HEAD must be pushed. Counts as one of the three GPU runs allowed per task.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
IID="$1"; BUCKET="$2"; MODEL="${3:-qwen3.5-4b}"
OUT="${OUT:-$HERE/soak-out}"; mkdir -p "$OUT"
WAIT="${WAIT:-9000}"
SHA="$(cd "$HERE" && git rev-parse HEAD)"

if [ -z "$(cd "$HERE" && git branch -r --contains "$SHA" 2>/dev/null)" ]; then
  echo "HEAD $SHA is not on origin; push it first (the box fetches the scripts by hash)." >&2
  exit 2
fi

KEY="soak-$IID-$(date +%s).zip"
PUT="$(python3 -c "import boto3;print(boto3.client('s3').generate_presigned_url('put_object',Params={'Bucket':'$BUCKET','Key':'$KEY'},ExpiresIn=14400))")"
RAW="https://raw.githubusercontent.com/PlugableTechnologies/plugable-chat/$SHA"

"$HERE/ssm.sh" "$IID" "
\$ProgressPreference='SilentlyContinue'
New-Item -ItemType Directory -Force C:\\gpu\\ci | Out-Null
foreach (\$f in 'CleanHost.psm1','check-chat-store-integrity.mjs') { Invoke-WebRequest -UseBasicParsing -Uri ('$RAW/scripts/ci/' + \$f) -OutFile ('C:\\gpu\\ci\\' + \$f) }
foreach (\$f in 'soak-run.ps1','run-in-session.ps1') { Invoke-WebRequest -UseBasicParsing -Uri ('$RAW/infra/aws-gpu/' + \$f) -OutFile ('C:\\gpu\\' + \$f) }
powershell -NoProfile -ExecutionPolicy Bypass -File C:\\gpu\\run-in-session.ps1 -Script C:\\gpu\\soak-run.ps1 -Args '-Model $MODEL' -TimeoutSec $WAIT
Compress-Archive -Path C:\\gpu\\soak\\* -DestinationPath C:\\gpu\\soak.zip -Force
Invoke-WebRequest -UseBasicParsing -Method Put -InFile C:\\gpu\\soak.zip -Uri '$PUT' | Out-Null
Get-Content C:\\gpu\\soak\\soak-result.txt -Tail 40" $((WAIT + 600)) || true

aws s3 cp "s3://$BUCKET/$KEY" "$OUT/soak.zip" --only-show-errors
unzip -o -q "$OUT/soak.zip" -d "$OUT/soak"
cat "$OUT/soak/verdict.txt"
grep -q 'soak=PASSED' "$OUT/soak/verdict.txt"
