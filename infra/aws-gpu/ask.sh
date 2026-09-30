#!/usr/bin/env bash
# Ask the installed app on the GPU box the Chicago crimes questions, one fresh app launch each,
# and bring each screenshot back so it can be checked against the expected answers.
#
#   ./ask.sh <instance-id> <model-id> <s3-bucket> [question-name ...]
#
# Environment:
#   WAIT=900      ceiling in seconds per question; the box script returns as soon as the app
#                 logs that the chat finished, so this is not a delay
#   NO_WARM=1     skip the one warm-up launch (first launches after install can spend minutes
#                 registering GPU providers; the warm-up absorbs that before the questions)
#   NO_SYNC=1     skip copying models from the SYSTEM cache (where the compiled tests download
#                 them) to the Administrator cache (where the installed app looks)
#   OUT=./ask-out where screenshots and summary.txt land
#
# Needs python3 + boto3 (to presign the upload) and the box's temporary SSM role. Expected
# answers are in chicago-questions.json, computed from the database itself.
#
# The scripts run on the box are fetched by COMMIT HASH from GitHub, so HEAD must be pushed.
# On exit (also on Ctrl-C) the box is told to stop any ask-app.ps1 and the app, because
# killing this script alone leaves the box-side copies running.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
IID="$1"; MODEL="$2"; BUCKET="$3"; shift 3
OUT="${OUT:-$HERE/ask-out}"; mkdir -p "$OUT"
WAIT="${WAIT:-900}"
SHA="$(cd "$HERE" && git rev-parse HEAD)"
RAW="https://raw.githubusercontent.com/PlugableTechnologies/plugable-chat/$SHA/infra/aws-gpu"

if [ -z "$(cd "$HERE" && git branch -r --contains "$SHA" 2>/dev/null)" ]; then
  echo "HEAD $SHA is not on origin; push it first (the box fetches the scripts by hash)." >&2
  exit 2
fi

NAMES=("$@")
if [ ${#NAMES[@]} -eq 0 ]; then
  read -r -a NAMES <<<"$(python3 -c "import json;print(' '.join(q['name'] for q in json.load(open('$HERE/chicago-questions.json'))))")"
fi

box() { # box <powershell> [timeout-seconds]: run on the box, print output, never abort this script
  "$HERE/ssm.sh" "$IID" "$1" "${2:-900}" 2>&1 || true
}

cleanup() {
  trap - EXIT INT TERM
  echo "--- cleaning up the box (stop ask-app.ps1 and the app)"
  "$HERE/ssm.sh" "$IID" "if (Test-Path C:\\gpu\\ask-app.ps1) { powershell -NoProfile -ExecutionPolicy Bypass -File C:\\gpu\\ask-app.ps1 -CleanupOnly -Model x }" 90 >/dev/null 2>&1 || true
}
trap cleanup EXIT
trap 'exit 130' INT TERM

echo "--- fetching scripts at $SHA"
box "
\$ProgressPreference='SilentlyContinue'
foreach (\$f in 'ask-app.ps1','sync-model-cache.ps1') { Invoke-WebRequest -UseBasicParsing -Uri ('$RAW/' + \$f) -OutFile ('C:\\gpu\\' + \$f) }
'fetched'" 120 | tail -2

if [ "${NO_SYNC:-0}" != "1" ]; then
  echo "--- syncing model cache (SYSTEM -> Administrator)"
  box "powershell -NoProfile -ExecutionPolicy Bypass -File C:\\gpu\\sync-model-cache.ps1" 900 | tail -8
fi

if [ "${NO_WARM:-0}" != "1" ]; then
  echo "--- warm-up launch (provider registration, model load)"
  box "powershell -NoProfile -ExecutionPolicy Bypass -File C:\\gpu\\ask-app.ps1 -Warm -Model '$MODEL' -WaitSeconds $WAIT" $((WAIT + 120)) | tail -4
fi

: > "$OUT/summary.txt"
FAILED=0
for NAME in "${NAMES[@]}"; do
  QUESTION="$(python3 -c "import json;print(next(q['question'] for q in json.load(open('$HERE/chicago-questions.json')) if q['name']=='$NAME'))")"
  URL="$(python3 -c "import boto3;print(boto3.client('s3').generate_presigned_url('put_object',Params={'Bucket':'$BUCKET','Key':'ask-$IID-$NAME.png'},ExpiresIn=7200))")"
  echo "=== $NAME: $QUESTION"
  RESULT="$(box "powershell -NoProfile -ExecutionPolicy Bypass -File C:\\gpu\\ask-app.ps1 -Question '$QUESTION' -Model '$MODEL' -PutUrl '$URL' -WaitSeconds $WAIT" $((WAIT + 120)) | tail -4)"
  echo "$RESULT"
  echo "$NAME: $(echo "$RESULT" | grep -o 'outcome=.*' | head -1)" >> "$OUT/summary.txt"
  echo "$RESULT" | grep -q 'outcome=chat-finished' || FAILED=$((FAILED + 1))
  aws s3 cp "s3://$BUCKET/ask-$IID-$NAME.png" "$OUT/$NAME.png" --only-show-errors && echo "screenshot: $OUT/$NAME.png" || true
done
echo "--- summary ($OUT/summary.txt)"; cat "$OUT/summary.txt"
[ "$FAILED" -eq 0 ]
