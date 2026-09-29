#!/usr/bin/env bash
# Run PowerShell on the box through SSM and print the result.
# Only for hands-on debugging and the spike: the production workflow gives the
# box no IAM role, so it cannot use SSM (it uses presigned S3 links instead).
#
#   ./ssm.sh <instance-id> '<powershell>' [timeoutSeconds]
#
# Lessons: SSM output is cut off at ~24 KB (write big results to a file); a
# command that runs longer than the calling tool's own timeout should be started
# in the background; commands run in session 0, so anything needing the desktop
# goes through run-in-session.ps1.
set -euo pipefail
IID="$1"; SCRIPT="$2"; TIMEOUT="${3:-900}"
PARAMS="$(python3 -c "import json,sys;print(json.dumps({'commands':[sys.argv[1]],'executionTimeout':[sys.argv[2]]}))" "$SCRIPT" "$TIMEOUT")"
CID="$(aws ssm send-command --instance-ids "$IID" --document-name AWS-RunPowerShellScript \
  --parameters "$PARAMS" --timeout-seconds "$TIMEOUT" --query Command.CommandId --output text)"
STATUS=""
for _ in $(seq 1 $((TIMEOUT / 5 + 6))); do
  STATUS="$(aws ssm get-command-invocation --command-id "$CID" --instance-id "$IID" --query Status --output text 2>/dev/null || true)"
  case "$STATUS" in Success|Failed|Cancelled|TimedOut) break ;; esac
  sleep 5
done
echo "[status $STATUS]"
aws ssm get-command-invocation --command-id "$CID" --instance-id "$IID" \
  --query '[StandardOutputContent,StandardErrorContent]' --output text
[ "$STATUS" = "Success" ]
