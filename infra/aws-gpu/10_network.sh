#!/usr/bin/env bash
# Isolated network for the GPU test box: one VPC, one public subnet, an internet
# gateway and a security group with NO inbound rules. All of these are free.
# Deliberately absent: NAT gateway and Elastic IP (both bill by the hour).
# Idempotent: re-running finds the existing VPC by its Project tag.
source "$(dirname "$0")/common.sh"

VPC_ID="$(find_vpc)"
if [ -z "$VPC_ID" ]; then
  VPC_ID="$(aws ec2 create-vpc --cidr-block "$VPC_CIDR" --tag-specifications "$(tag_spec vpc)" \
    --query Vpc.VpcId --output text)"
  aws ec2 modify-vpc-attribute --vpc-id "$VPC_ID" --enable-dns-hostnames
  echo "created vpc $VPC_ID"
fi

SUBNET_ID="$(aws ec2 describe-subnets --filters "Name=vpc-id,Values=$VPC_ID" --query 'Subnets[0].SubnetId' --output text | sed 's/^None$//')"
if [ -z "$SUBNET_ID" ]; then
  SUBNET_ID="$(aws ec2 create-subnet --vpc-id "$VPC_ID" --cidr-block "$VPC_CIDR" --availability-zone "$SUBNET_AZ" \
    --tag-specifications "$(tag_spec subnet)" --query Subnet.SubnetId --output text)"
  aws ec2 modify-subnet-attribute --subnet-id "$SUBNET_ID" --map-public-ip-on-launch
  echo "created subnet $SUBNET_ID"
fi

IGW_ID="$(aws ec2 describe-internet-gateways --filters "Name=attachment.vpc-id,Values=$VPC_ID" --query 'InternetGateways[0].InternetGatewayId' --output text | sed 's/^None$//')"
if [ -z "$IGW_ID" ]; then
  IGW_ID="$(aws ec2 create-internet-gateway --tag-specifications "$(tag_spec internet-gateway)" \
    --query InternetGateway.InternetGatewayId --output text)"
  aws ec2 attach-internet-gateway --internet-gateway-id "$IGW_ID" --vpc-id "$VPC_ID"
  echo "created internet gateway $IGW_ID"
fi

RTB_ID="$(aws ec2 describe-route-tables --filters "Name=vpc-id,Values=$VPC_ID" "Name=association.main,Values=true" \
  --query 'RouteTables[0].RouteTableId' --output text)"
aws ec2 create-route --route-table-id "$RTB_ID" --destination-cidr-block 0.0.0.0/0 --gateway-id "$IGW_ID" >/dev/null 2>&1 || true

SG_ID="$(aws ec2 describe-security-groups --filters "Name=vpc-id,Values=$VPC_ID" "Name=group-name,Values=$SG_NAME" \
  --query 'SecurityGroups[0].GroupId' --output text | sed 's/^None$//')"
if [ -z "$SG_ID" ]; then
  SG_ID="$(aws ec2 create-security-group --group-name "$SG_NAME" --vpc-id "$VPC_ID" \
    --description "plugable-chat GPU test box: no inbound, outbound only" \
    --tag-specifications "$(tag_spec security-group)" --query GroupId --output text)"
  echo "created security group $SG_ID"
fi

echo "VPC_ID=$VPC_ID SUBNET_ID=$SUBNET_ID SG_ID=$SG_ID"
