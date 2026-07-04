#!/usr/bin/env bash
# Apply Tendril's branch-protection rulesets via the Gitea API.
#
# Two long-lived branches (see CONTRIBUTING.md):
#   main — release/stable. No direct pushes; changes only via PR from dev. Strict.
#   dev  — default integration branch. Direct pushes allowed; force-push and deletion blocked.
#
# Branch protection can't be set over SSH — it needs an API token. Create one in Gitea:
#   User Settings -> Applications -> Generate New Token  (scope: repository read+write)
#
# Usage:
#   GITEA_TOKEN=xxxxx ./scripts/setup-branch-protection.sh
#
# Optional env overrides:
#   GITEA_HOST (default git.onetick.ninja)  OWNER (flan)  REPO (tendril)
#   MAIN_REQUIRED_APPROVALS (default 0 — set 1 once there's more than one maintainer)
#   ENABLE_STATUS_CHECK (default false — set true once a Gitea Actions runner is registered)
set -euo pipefail

: "${GITEA_TOKEN:?Set GITEA_TOKEN (Gitea API token with repo read+write scope)}"
HOST="${GITEA_HOST:-git.onetick.ninja}"
OWNER="${OWNER:-flan}"
REPO="${REPO:-tendril}"
MAIN_REQUIRED_APPROVALS="${MAIN_REQUIRED_APPROVALS:-0}"
ENABLE_STATUS_CHECK="${ENABLE_STATUS_CHECK:-false}"

API="https://${HOST}/api/v1/repos/${OWNER}/${REPO}/branch_protections"

# apply_rule <branch> <json-body>: POST, and PATCH if the rule already exists.
apply_rule() {
  local branch="$1" body="$2"
  echo "Applying protection to ${OWNER}/${REPO}@${branch}"
  local code
  code=$(curl -s -o /tmp/bp.out -w '%{http_code}' -X POST "${API}" \
    -H "Authorization: token ${GITEA_TOKEN}" \
    -H "Content-Type: application/json" -d "${body}")
  if [ "${code}" = "409" ] || [ "${code}" = "422" ]; then
    echo "  rule exists — updating (PATCH)"
    curl -s -o /tmp/bp.out -w '  PATCH %{http_code}\n' -X PATCH "${API}/${branch}" \
      -H "Authorization: token ${GITEA_TOKEN}" \
      -H "Content-Type: application/json" -d "${body}"
  else
    echo "  POST ${code}"
  fi
}

# main — strict: no direct push, PR-only, up-to-date required.
apply_rule main "$(cat <<JSON
{
  "rule_name": "main",
  "enable_push": false,
  "required_approvals": ${MAIN_REQUIRED_APPROVALS},
  "dismiss_stale_approvals": true,
  "block_on_rejected_reviews": true,
  "block_on_official_review_requests": true,
  "block_on_outdated_branch": true,
  "require_signed_commits": false,
  "enable_status_check": ${ENABLE_STATUS_CHECK},
  "status_check_contexts": ["ci"]
}
JSON
)"

# dev — integration: direct pushes allowed, force-push and deletion blocked.
apply_rule dev "$(cat <<JSON
{
  "rule_name": "dev",
  "enable_push": true,
  "required_approvals": 0,
  "dismiss_stale_approvals": true,
  "block_on_rejected_reviews": true,
  "block_on_outdated_branch": false,
  "enable_status_check": ${ENABLE_STATUS_CHECK},
  "status_check_contexts": ["ci"]
}
JSON
)"

echo "Done. Default branch should be 'dev' (set via: PATCH /repos/${OWNER}/${REPO} {\"default_branch\":\"dev\"})."
