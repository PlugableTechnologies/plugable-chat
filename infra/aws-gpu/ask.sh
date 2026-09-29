#!/usr/bin/env bash
# Ask the installed app on the GPU box the Chicago crimes questions, one fresh app launch each,
# and bring each screenshot back so it can be checked against the expected answers.
#
#   ./ask.sh <instance-id> <model-id> <s3-bucket> [question-name ...]
#
# Needs python3 + boto3 (to presign the upload) and the box's temporary SSM role.
# Screenshots land in $OUT (default ./ask-out) as <name>.png. Expected answers are in
# chicago-questions.json, computed from the database itself.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
IID="$1"; MODEL="$2"; BUCKET="$3"; shift 3
OUT="${OUT:-$HERE/ask-out}"; mkdir -p "$OUT"
SHA="$(cd "$HERE" && git rev-parse HEAD)"
NAMES=("$@")
if [ ${#NAMES[@]} -eq 0 ]; then
  read -r -a NAMES <<<"$(python3 -c "import json;print(' '.join(q['name'] for q in json.load(open('$HERE/chicago-questions.json'))))")"
fi
for NAME in "${NAMES[@]}"; do
  QUESTION="$(python3 -c "import json;print(next(q['question'] for q in json.load(open('$HERE/chicago-questions.json')) if q['name']=='$NAME'))")"
  URL="$(python3 -c "import boto3;print(boto3.client('s3').generate_presigned_url('put_object',Params={'Bucket':'$BUCKET','Key':'ask-$NAME.png'},ExpiresIn=3600))")"
  echo "=== $NAME: $QUESTION"
  "$HERE/ssm.sh" "$IID" "
\$ProgressPreference='SilentlyContinue'
Invoke-WebRequest -UseBasicParsing 'https://raw.githubusercontent.com/PlugableTechnologies/plugable-chat/$SHA/infra/aws-gpu/ask-app.ps1' -OutFile C:\\gpu\\ask-app.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File C:\\gpu\\ask-app.ps1 -Question '$QUESTION' -Model '$MODEL' -PutUrl '$URL'
" 600 2>&1 | tail -4
  aws s3 cp "s3://$BUCKET/ask-$NAME.png" "$OUT/$NAME.png" --only-show-errors && echo "screenshot: $OUT/$NAME.png"
done
