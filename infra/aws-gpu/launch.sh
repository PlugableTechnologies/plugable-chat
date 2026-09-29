#!/usr/bin/env bash
# Launch one fresh GPU test box from Amazon's stock Windows image and print its id.
# Nothing is saved between runs: no AMI, no snapshot; the disk is deleted on
# termination and the instance terminates when it shuts down.
#
#   ./launch.sh                # production shape: no IAM role on the instance
#   SPIKE_SSM=1 ./launch.sh    # hands-on debugging: adds a temporary SSM role
#
# Prerequisites: ./10_network.sh (VPC, subnet, security group).
source "$(dirname "$0")/common.sh"

AMI="$(aws ssm get-parameter --name "$AMI_SSM_PARAM" --query Parameter.Value --output text)"
VPC_ID="$(find_vpc)"; [ -n "$VPC_ID" ] || { echo "run ./10_network.sh first" >&2; exit 1; }
SUBNET_ID="$(aws ec2 describe-subnets --filters "Name=vpc-id,Values=$VPC_ID" --query 'Subnets[0].SubnetId' --output text)"
SG_ID="$(aws ec2 describe-security-groups --filters "Name=vpc-id,Values=$VPC_ID" "Name=group-name,Values=$SG_NAME" --query 'SecurityGroups[0].GroupId' --output text)"

EXTRA=()
if [ "${SPIKE_SSM:-0}" = "1" ]; then
  ROLE=plugable-chat-gpu-spike
  if ! aws iam get-role --role-name "$ROLE" >/dev/null 2>&1; then
    aws iam create-role --role-name "$ROLE" --tags "Key=$PROJECT_TAG_KEY,Value=$PROJECT_TAG_VALUE" \
      --assume-role-policy-document '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}' >/dev/null
    aws iam attach-role-policy --role-name "$ROLE" --policy-arn arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore
    aws iam create-instance-profile --instance-profile-name "$ROLE" >/dev/null
    aws iam add-role-to-instance-profile --instance-profile-name "$ROLE" --role-name "$ROLE"
    sleep 12   # IAM propagation
  fi
  EXTRA+=(--iam-instance-profile "Name=$ROLE")
fi

# A 100 GB root disk: the OS, driver, app and a model or two. gp3, deleted on termination.
IID="$(aws ec2 run-instances --image-id "$AMI" --instance-type "$INSTANCE_TYPE" \
  --subnet-id "$SUBNET_ID" --security-group-ids "$SG_ID" "${EXTRA[@]}" \
  --instance-initiated-shutdown-behavior terminate \
  --block-device-mappings 'DeviceName=/dev/sda1,Ebs={VolumeSize=100,VolumeType=gp3,DeleteOnTermination=true}' \
  --metadata-options HttpTokens=required \
  --tag-specifications "$(tag_spec instance)" "$(tag_spec volume)" \
  --query 'Instances[0].InstanceId' --output text)"
# Hard stop enforced by AWS itself, created at once. If the guest hangs or its own failsafe is
# lost, AWS still terminates the box at this time. MAX_RUN_MINUTES (default 180) sets it.
create_terminate_schedule "$IID" "$MAX_RUN_MINUTES"
echo "$IID"
