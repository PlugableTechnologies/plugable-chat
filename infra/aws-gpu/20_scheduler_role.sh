#!/usr/bin/env bash
# One-time setup for the AWS-side hard time limit: an IAM role that EventBridge Scheduler uses
# to terminate GPU test boxes, and a schedule group. Both are free. The role can only terminate
# instances tagged Project=plugable-chat-gpu, so it cannot touch anything else in the account.
source "$(dirname "$0")/common.sh"

TRUST='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"scheduler.amazonaws.com"},"Action":"sts:AssumeRole"}]}'
POLICY='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"ec2:TerminateInstances","Resource":"arn:aws:ec2:*:*:instance/*","Condition":{"StringEquals":{"aws:ResourceTag/Project":"plugable-chat-gpu"}}}]}'

if ! aws iam get-role --role-name "$SCHEDULER_ROLE_NAME" >/dev/null 2>&1; then
  aws iam create-role --role-name "$SCHEDULER_ROLE_NAME" --assume-role-policy-document "$TRUST" \
    --tags "Key=$PROJECT_TAG_KEY,Value=$PROJECT_TAG_VALUE" >/dev/null
  echo "created role $SCHEDULER_ROLE_NAME"
fi
aws iam put-role-policy --role-name "$SCHEDULER_ROLE_NAME" --policy-name terminate-tagged-gpu-boxes --policy-document "$POLICY"

if ! aws scheduler get-schedule-group --name "$SCHEDULE_GROUP" >/dev/null 2>&1; then
  aws scheduler create-schedule-group --name "$SCHEDULE_GROUP" --tags "Key=$PROJECT_TAG_KEY,Value=$PROJECT_TAG_VALUE" >/dev/null
  echo "created schedule group $SCHEDULE_GROUP"
fi
sleep 10   # IAM propagation before the first schedule uses the role
echo "ok"
