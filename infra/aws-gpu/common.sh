#!/usr/bin/env bash
# Shared settings for the on-demand Windows GPU test box (see docs/gpu-validation.md).
# Nothing here is a secret. Every AWS resource this project creates carries the
# Project tag below, so a leftover check can find it.
set -euo pipefail

export AWS_REGION="${AWS_REGION:-us-east-1}"
export AWS_DEFAULT_REGION="$AWS_REGION"

PROJECT_TAG_KEY="Project"
PROJECT_TAG_VALUE="plugable-chat-gpu"
VPC_NAME="plugable-chat-gpu"
VPC_CIDR="10.60.0.0/24"
SUBNET_AZ="${SUBNET_AZ:-us-east-1a}"   # g4dn.xlarge is offered in 1a-1d and 1f
SG_NAME="plugable-chat-gpu-no-inbound"
INSTANCE_TYPE="${INSTANCE_TYPE:-g5.xlarge}"  # A10G (Ampere); Turing (T4) and older are not supported test targets
# Server 2025 (build 26100, the same base as Windows 11 24H2): Windows ML's hardware-optimized
# execution providers need 24H2 or newer. Use ...Windows_Server-2022-... to test the older floor.
AMI_SSM_PARAM="${AMI_SSM_PARAM:-/aws/service/ami-windows-latest/Windows_Server-2025-English-Full-Base}"
BUDGET_NAME="plugable-chat-gpu-monthly"
BUDGET_LIMIT_USD="${BUDGET_LIMIT_USD:-25}"
BUDGET_EMAIL="${BUDGET_EMAIL:-bernie@plugable.com}"

tag_spec() { # tag_spec <resource-type> [extra Key=..,Value=.. pairs]
  local rt="$1"; shift
  echo "ResourceType=${rt},Tags=[{Key=${PROJECT_TAG_KEY},Value=${PROJECT_TAG_VALUE}},{Key=Name,Value=${VPC_NAME}}$(printf ',{%s}' "$@")]"
}

find_vpc() {
  aws ec2 describe-vpcs --filters "Name=tag:${PROJECT_TAG_KEY},Values=${PROJECT_TAG_VALUE}" \
    --query 'Vpcs[0].VpcId' --output text | sed 's/^None$//'
}

# ---- Hard time limit, enforced by AWS (not by the guest) ---------------------------------
# The guest has its own failsafe (bootstrap-box.ps1), but a hung or mis-armed guest cannot be
# trusted to stop itself. Every box also gets a one-time EventBridge Scheduler entry that calls
# ec2:TerminateInstances at a fixed time. The entry deletes itself after it runs, so nothing
# is left between runs. Scheduler is free at this volume.
SCHEDULER_ROLE_NAME="plugable-chat-gpu-scheduler"
SCHEDULE_GROUP="plugable-chat-gpu"
MAX_RUN_MINUTES="${MAX_RUN_MINUTES:-180}"

utc_in_minutes() { # utc_in_minutes <minutes>  -> 2026-09-29T13:00:00
  python3 -c "import datetime,sys;print((datetime.datetime.now(datetime.timezone.utc)+datetime.timedelta(minutes=int(sys.argv[1]))).strftime('%Y-%m-%dT%H:%M:%S'))" "$1"
}

# create_terminate_schedule <instance-id> <minutes-from-now>; replaces an existing entry.
create_terminate_schedule() {
  local iid="$1" minutes="$2" name="gpu-terminate-$1" role when
  role="$(aws iam get-role --role-name "$SCHEDULER_ROLE_NAME" --query Role.Arn --output text)"
  when="$(utc_in_minutes "$minutes")"
  aws scheduler delete-schedule --name "$name" --group-name "$SCHEDULE_GROUP" >/dev/null 2>&1 || true
  aws scheduler create-schedule --name "$name" --group-name "$SCHEDULE_GROUP" \
    --schedule-expression "at($when)" --schedule-expression-timezone UTC \
    --flexible-time-window Mode=OFF --action-after-completion DELETE \
    --description "Hard stop for GPU test box $iid" \
    --target "{\"Arn\":\"arn:aws:scheduler:::aws-sdk:ec2:terminateInstances\",\"RoleArn\":\"$role\",\"Input\":\"{\\\"InstanceIds\\\":[\\\"$iid\\\"]}\"}" >/dev/null
  echo "AWS will terminate $iid at $when UTC (in $minutes min)" >&2
}
