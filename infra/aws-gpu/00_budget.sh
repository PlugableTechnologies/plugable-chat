#!/usr/bin/env bash
# Monthly cost alarm for the GPU test box. Free to create. Alerts by e-mail at
# 50% and 100% of the limit (actual) and 100% (forecast).
#
# The filter is the Project cost-allocation tag. AWS only starts filtering by a
# tag after it is ACTIVATED in Billing (and has appeared on a bill line), which
# can lag a day; until then this budget reads $0. 20_activate_cost_tag.sh does
# the activation once the first resources exist.
source "$(dirname "$0")/common.sh"
ACCOUNT_ID="$(aws sts get-caller-identity --query Account --output text)"

if aws budgets describe-budget --account-id "$ACCOUNT_ID" --budget-name "$BUDGET_NAME" >/dev/null 2>&1; then
  echo "budget $BUDGET_NAME already exists"; exit 0
fi

notif() { # notif <type> <threshold>
  echo "{\"Notification\":{\"NotificationType\":\"$1\",\"ComparisonOperator\":\"GREATER_THAN\",\"Threshold\":$2,\"ThresholdType\":\"PERCENTAGE\"},\"Subscribers\":[{\"SubscriptionType\":\"EMAIL\",\"Address\":\"${BUDGET_EMAIL}\"}]}"
}

aws budgets create-budget --account-id "$ACCOUNT_ID" \
  --budget "{\"BudgetName\":\"${BUDGET_NAME}\",\"BudgetLimit\":{\"Amount\":\"${BUDGET_LIMIT_USD}\",\"Unit\":\"USD\"},\"TimeUnit\":\"MONTHLY\",\"BudgetType\":\"COST\",\"CostFilters\":{\"TagKeyValue\":[\"user:${PROJECT_TAG_KEY}\$${PROJECT_TAG_VALUE}\"]}}" \
  --notifications-with-subscribers "$(notif ACTUAL 50)" "$(notif ACTUAL 100)" "$(notif FORECASTED 100)"
echo "created budget $BUDGET_NAME: \$${BUDGET_LIMIT_USD}/month, alerts to ${BUDGET_EMAIL}"
