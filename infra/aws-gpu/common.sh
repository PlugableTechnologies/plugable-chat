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
INSTANCE_TYPE="${INSTANCE_TYPE:-g4dn.xlarge}"
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
