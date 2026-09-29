#!/usr/bin/env bash
# Move a box's hard stop to <minutes> from NOW, on both the AWS side and inside the guest.
#
#   ./extend.sh <instance-id> <minutes>
#
# AWS-side: replaces the one-time terminate schedule. Guest-side: rewrites C:\gpu\deadline.txt
# (needs the box's SSM role, so it works for boxes launched with SPIKE_SSM=1). Never extends past
# what you ask for: run it again to extend again.
source "$(dirname "$0")/common.sh"
IID="$1"; MINUTES="$2"
create_terminate_schedule "$IID" "$MINUTES"
"$(dirname "$0")/ssm.sh" "$IID" "(Get-Date).ToUniversalTime().AddMinutes($MINUTES).ToString('o') | Set-Content C:\\gpu\\deadline.txt; 'guest deadline: ' + (Get-Content C:\\gpu\\deadline.txt)" 60 || echo "guest deadline not updated (no SSM); the AWS-side stop still applies" >&2
