#!/usr/bin/env bash
# Apply Tendril's branch-protection ruleset to `main` on Gitea via the API.
#
# Branch protection can't be set over SSH — it needs an API token. Create one in Gitea:
#   User Settings -> Applications -> Generate New Token  (scope: repository read+write)
#
# Usage:
#   GITEA_TOKEN=xxxxx ./scripts/setup-branch-protection.sh
#
# Optional env overrides:
#   GITEA_HOST (default git.onetick.ninja)  OWNER (flan)  REPO (tendril)  BRANCH (main)
#   REQUIRED_APPROVALS (default 1 — set 0 for a solo maintainer who can't approve their own PRs)
#   ENABLE_STATUS_CHECK (default false — set true once a Gitea Actions runner is registered)
set -euo pipefail

: "${GITEA_TOKEN:?Set GITEA_TOKEN (Gitea API token with repo read+write scope)}"
HOST="${GITEA_HOST:-git.onetick.ninja}"
OWNER="${OWNER:-flan}"
REPO="${REPO:-tendril}"
BRANCH="${BRANCH:-main}"
REQUIRED_APPROVALS="${REQUIRED_APPROVALS:-1}"
ENABLE_STATUS_CHECK="${ENABLE_STATUS_CHECK:-false}"

API="https://${HOST}/api/v1/repos/${OWNER}/${REPO}/branch_protections"

read -r -d '' BODY <<JSON || true
{
  "rule_name": "${BRANCH}",
  "enable_push": false,
  "enable_push_whitelist": false,
  "required_approvals": ${REQUIRED_APPROVALS},
  "dismiss_stale_approvals": true,
  "block_on_rejected_reviews": true,
  "block_on_official_review_requests": true,
  "block_on_outdated_branch": true,
  "require_signed_commits": false,
  "enable_status_check": ${ENABLE_STATUS_CHECK},
  "status_check_contexts": ["ci"]
}
JSON

echo "Applying protection to ${OWNER}/${REPO}@${BRANCH} (approvals=${REQUIRED_APPROVALS}, status_check=${ENABLE_STATUS_CHECK})"

# Try to create; if a rule already exists, patch it.
code=$(curl -s -o /tmp/bp.out -w '%{http_code}' -X POST "${API}" \
  -H "Authorization: token ${GITEA_TOKEN}" \
  -H "Content-Type: application/json" -d "${BODY}")

if [ "${code}" = "409" ] || [ "${code}" = "422" ]; then
  echo "Rule exists — updating."
  curl -s -o /tmp/bp.out -w 'PATCH %{http_code}\n' -X PATCH "${API}/${BRANCH}" \
    -H "Authorization: token ${GITEA_TOKEN}" \
    -H "Content-Type: application/json" -d "${BODY}"
else
  echo "POST ${code}"
fi

echo "--- response ---"; cat /tmp/bp.out; echo
