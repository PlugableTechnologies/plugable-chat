#!/usr/bin/env bash
# Terminate the box(es), remove the temporary spike role, then PROVE nothing
# billable remains. Exits non-zero if anything tagged for this project is left.
#
#   ./teardown.sh [instance-id ...]     # no ids: terminate every tagged instance
source "$(dirname "$0")/common.sh"

TAG_FILTER="Name=tag:${PROJECT_TAG_KEY},Values=${PROJECT_TAG_VALUE}"
IDS=("$@")
if [ ${#IDS[@]} -eq 0 ]; then
  read -r -a IDS <<<"$(aws ec2 describe-instances --filters "$TAG_FILTER" \
    "Name=instance-state-name,Values=pending,running,stopping,stopped" \
    --query 'Reservations[].Instances[].InstanceId' --output text)"
fi
if [ ${#IDS[@]} -gt 0 ] && [ -n "${IDS[0]}" ]; then
  aws ec2 terminate-instances --instance-ids "${IDS[@]}" >/dev/null
  aws ec2 wait instance-terminated --instance-ids "${IDS[@]}"
  echo "terminated: ${IDS[*]}"
fi

ROLE=plugable-chat-gpu-spike
if aws iam get-role --role-name "$ROLE" >/dev/null 2>&1; then
  aws iam remove-role-from-instance-profile --instance-profile-name "$ROLE" --role-name "$ROLE" 2>/dev/null || true
  aws iam delete-instance-profile --instance-profile-name "$ROLE" 2>/dev/null || true
  aws iam detach-role-policy --role-name "$ROLE" --policy-arn arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore 2>/dev/null || true
  aws iam delete-role --role-name "$ROLE"
  echo "deleted temporary role $ROLE"
fi

# The proof. Each of these bills while it exists (or, for instances, until terminated).
LEFT=0
report() { # report <what> <count>
  if [ "$2" != "0" ] && [ -n "$2" ]; then echo "LEFTOVER $1: $2"; LEFT=1; else echo "ok  no $1"; fi
}
report "running/stopped instances" "$(aws ec2 describe-instances --filters "$TAG_FILTER" \
  "Name=instance-state-name,Values=pending,running,stopping,stopped" --query 'length(Reservations[].Instances[])' --output text)"
report "volumes" "$(aws ec2 describe-volumes --filters "$TAG_FILTER" --query 'length(Volumes)' --output text)"
report "snapshots" "$(aws ec2 describe-snapshots --owner-ids self --filters "$TAG_FILTER" --query 'length(Snapshots)' --output text)"
report "images" "$(aws ec2 describe-images --owners self --filters "$TAG_FILTER" --query 'length(Images)' --output text)"
report "elastic IPs" "$(aws ec2 describe-addresses --filters "$TAG_FILTER" --query 'length(Addresses)' --output text)"
report "NAT gateways" "$(aws ec2 describe-nat-gateways --filter "Name=tag:${PROJECT_TAG_KEY},Values=${PROJECT_TAG_VALUE}" "Name=state,Values=pending,available" --query 'length(NatGateways)' --output text)"
report "S3 buckets" "$(aws s3api list-buckets --query "length(Buckets[?starts_with(Name, 'plugable-chat-gpu-')])" --output text)"
exit $LEFT
