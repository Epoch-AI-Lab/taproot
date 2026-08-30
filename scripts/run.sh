#!/usr/bin/env bash
set -euo pipefail

STATE_PATH="${INPUT_STATE_PATH:-.taproot/state.json}"
BASELINE_REF="${INPUT_BASELINE_REF:-${GITHUB_BASE_REF:-main}}"
BASELINE_PATH="${INPUT_BASELINE_PATH:-$STATE_PATH}"
FAIL_ON_DRIFT="${INPUT_FAIL_ON_DRIFT:-true}"
WORKDIR="${INPUT_WORKING_DIRECTORY:-.}"
REGISTRY_URL="${INPUT_REGISTRY_URL:-}"
REGISTRY_TOKEN="${INPUT_REGISTRY_TOKEN:-}"
REGISTRY_PATH="${INPUT_REGISTRY_PATH:-.taproot/registry}"

# Validate baseline ref to prevent git argument injection
if [[ "$BASELINE_REF" == -* ]] || [[ "$BASELINE_REF" == *$'\n'* ]]; then
  echo "::error::Invalid baseline-ref: must not start with '-' or contain newline"
  exit 2
fi
if [[ "$BASELINE_PATH" == *$'\n'* ]]; then
  echo "::error::Invalid baseline-path: must not contain newline"
  exit 2
fi

cd "$WORKDIR"

ACTION_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# Temp files with trap cleanup (fix M2)
BASELINE_TMP="$(mktemp)"
CHECK_JSON="$(mktemp)"
CHECK_LOG="$(mktemp)"
trap 'rm -f "$BASELINE_TMP" "$CHECK_JSON" "$CHECK_LOG"' EXIT

# Escape helpers for workflow commands (fix H1/H2)
escape_wf() {
  local s="$1"
  s="${s//%/%25}"
  s="${s//$'\r'/%0D}"
  s="${s//$'\n'/%0A}"
  printf '%s' "$s"
}
escape_wf_colon() {
  local s
  s="$(escape_wf "$1")"
  s="${s//:/%3A}"
  printf '%s' "$s"
}

# Ensure taproot binary available
TAPROOT_BIN=""
if [[ -x "$ACTION_DIR/target/debug/taproot" ]]; then
  TAPROOT_BIN="$ACTION_DIR/target/debug/taproot"
elif command -v taproot &>/dev/null; then
  TAPROOT_BIN="taproot"
else
  echo "::group::Installing taproot"
  if command -v cargo &>/dev/null; then
    cargo install --path "$ACTION_DIR" --locked 2>&1 | tail -5
    TAPROOT_BIN="taproot"
  else
    echo "::error::taproot binary not found and cargo not available"
    exit 2
  fi
  echo "::endgroup::"
fi

echo "Taproot baseline check (strict)"
echo "  state:    $STATE_PATH"
echo "  baseline: $BASELINE_REF:$BASELINE_PATH"
echo "  binary:   $TAPROOT_BIN"
echo ""

# 1. Verify head exists and is signed + valid
if [[ ! -f "$STATE_PATH" ]]; then
  esc_path="$(escape_wf "$STATE_PATH")"
  echo "::error file=$esc_path::Head state not found at $esc_path"
  echo "drift=true" >> "$GITHUB_OUTPUT" 2>/dev/null || true
  echo "verified=false" >> "$GITHUB_OUTPUT" 2>/dev/null || true
  echo "baseline-exists=false" >> "$GITHUB_OUTPUT" 2>/dev/null || true
  exit 1
fi

echo "::group::Verify head state"
if ! "$TAPROOT_BIN" verify --state-path "$STATE_PATH"; then
  esc_path="$(escape_wf "$STATE_PATH")"
  echo "::error file=$esc_path::Head state verification failed (hash or signature invalid)"
  echo "verified=false" >> "$GITHUB_OUTPUT" 2>/dev/null || true
  if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    echo "hash=" >> "$GITHUB_OUTPUT"
    echo "drift=true" >> "$GITHUB_OUTPUT"
    echo "baseline-exists=false" >> "$GITHUB_OUTPUT"
  fi
  exit 1
fi
HEAD_HASH=$("$TAPROOT_BIN" verify --state-path "$STATE_PATH" 2>&1 | grep -oE 'sha256:[a-f0-9]+' | cut -d: -f2 || echo "")
echo "verified=true" >> "$GITHUB_OUTPUT" 2>/dev/null || true
echo "hash=$HEAD_HASH" >> "$GITHUB_OUTPUT" 2>/dev/null || true
echo "::endgroup::"

# 2. Fetch baseline — try remote registry first, then git
BASELINE_EXISTS="false"
BASELINE_HASH=""

# Try remote registry if configured
REMOTE_BASELINE_FETCHED="false"
if [[ -n "$REGISTRY_URL" ]]; then
  echo "::group::Try remote baseline from $REGISTRY_URL"
  # Derive repo/branch from state file if possible, else use baseline ref as branch
  REPO_HINT=$(jq -r '.state.base.repo // empty' "$STATE_PATH" 2>/dev/null || echo "")
  BRANCH_HINT="$BASELINE_REF"
  if [[ -n "$REPO_HINT" && -n "$BRANCH_HINT" ]]; then
    ENCODED_REPO=$(echo "$REPO_HINT" | sed 's/%/%25/g; s|/|%2F|g')
    ENCODED_BRANCH=$(echo "$BRANCH_HINT" | sed 's/%/%25/g; s|/|%2F|g')
    REMOTE_REF_URL="${REGISTRY_URL%/}/v1/refs/$ENCODED_REPO/$ENCODED_BRANCH"
    AUTH_HEADER=""
    if [[ -n "$REGISTRY_TOKEN" ]]; then
      AUTH_HEADER="Authorization: Bearer $REGISTRY_TOKEN"
    fi
    if REMOTE_HASH=$(curl -s -f ${AUTH_HEADER:+-H "$AUTH_HEADER"} "$REMOTE_REF_URL" 2>/dev/null | jq -r '.hash // empty' 2>/dev/null); then
      if [[ -n "$REMOTE_HASH" && ${#REMOTE_HASH} -eq 64 ]]; then
        REMOTE_STATE_URL="${REGISTRY_URL%/}/v1/states/$REMOTE_HASH"
        if curl -s -f ${AUTH_HEADER:+-H "$AUTH_HEADER"} "$REMOTE_STATE_URL" -o "$BASELINE_TMP" 2>/dev/null; then
          if "$TAPROOT_BIN" verify --state-path "$BASELINE_TMP" >/dev/null 2>&1; then
            REMOTE_BASELINE_FETCHED="true"
            echo "✓ remote baseline fetched: $REMOTE_HASH"
          fi
        fi
      fi
    fi
  fi
  echo "::endgroup::"
  if [[ "$REMOTE_BASELINE_FETCHED" == "true" ]]; then
    BASELINE_EXISTS="true"
    echo "baseline-exists=true" >> "$GITHUB_OUTPUT" 2>/dev/null || true
    # Verify and extract hash via remote file
    echo "::group::Verify baseline state (remote)"
    if ! "$TAPROOT_BIN" verify --state-path "$BASELINE_TMP" 2>&1; then
      echo "::warning::Baseline state has invalid hash/signature — failing check"
      echo "baseline-hash=" >> "$GITHUB_OUTPUT" 2>/dev/null || true
    else
      BASELINE_HASH=$("$TAPROOT_BIN" verify --state-path "$BASELINE_TMP" 2>&1 | grep -oE 'sha256:[a-f0-9]+' | cut -d: -f2 || echo "")
      echo "baseline-hash=$BASELINE_HASH" >> "$GITHUB_OUTPUT" 2>/dev/null || true
    fi
    echo "::endgroup::"
    # Reuse same drift logic below — set flag to enter drift block
  fi
fi

# Fallback to git if remote not fetched
if [[ "$REMOTE_BASELINE_FETCHED" != "true" ]]; then
  git fetch --depth 1 origin -- "$BASELINE_REF" 2>/dev/null || true
  if git show "origin/$BASELINE_REF:$BASELINE_PATH" > "$BASELINE_TMP" 2>/dev/null; then
    BASELINE_EXISTS="true"
    echo "baseline-exists=true" >> "$GITHUB_OUTPUT" 2>/dev/null || true

    echo "::group::Verify baseline state"
    if ! "$TAPROOT_BIN" verify --state-path "$BASELINE_TMP" 2>&1; then
      echo "::warning::Baseline state has invalid hash/signature — failing check"
      echo "baseline-hash=" >> "$GITHUB_OUTPUT" 2>/dev/null || true
    else
      BASELINE_HASH=$("$TAPROOT_BIN" verify --state-path "$BASELINE_TMP" 2>&1 | grep -oE 'sha256:[a-f0-9]+' | cut -d: -f2 || echo "")
      echo "baseline-hash=$BASELINE_HASH" >> "$GITHUB_OUTPUT" 2>/dev/null || true
    fi
    echo "::endgroup::"
  fi
fi

# 2b. If baseline exists (from either remote or git), run drift check
if [[ "$BASELINE_EXISTS" == "true" ]]; then

  # 3. Diff — strict mode (fail on unsigned, fail on warnings)
  echo "::group::Taproot check (strict)"
  set +e
  "$TAPROOT_BIN" check --baseline "$BASELINE_TMP" --state-path "$STATE_PATH" --json > "$CHECK_JSON" 2>"$CHECK_LOG"
  CHECK_EXIT=$?
  set -e
  cat "$CHECK_LOG" 2>/dev/null || true
  if [[ -f "$CHECK_JSON" ]]; then
    cat "$CHECK_JSON"
    echo ""
    "$TAPROOT_BIN" check --baseline "$BASELINE_TMP" --state-path "$STATE_PATH" 2>&1 || true
  fi

  # Summary — escape baseline ref for markdown
  if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    {
      echo "## Taproot baseline check (strict)"
      echo ""
      echo "| | head | baseline ($BASELINE_REF) |"
      echo "|--|--|--|"
      echo "| hash | \`$HEAD_HASH\` | \`${BASELINE_HASH:-unknown}\` |"
      echo "| drift | $(if [[ $CHECK_EXIT -ne 0 ]]; then echo "✗ detected"; else echo "✓ none"; fi) | |"
      echo ""
      if [[ $CHECK_EXIT -ne 0 && -f "$CHECK_JSON" ]]; then
        echo "### Changes"
        echo '```json'
        cat "$CHECK_JSON"
        echo '```'
      fi
    } >> "$GITHUB_STEP_SUMMARY"
  fi

  # Annotations — escape % \r \n and : in workflow commands (fix H1)
  if [[ $CHECK_EXIT -ne 0 && -f "$CHECK_JSON" ]] && command -v jq &>/dev/null; then
    while IFS= read -r line; do
      esc_line="$(escape_wf "$line")"
      echo "::warning::$esc_line"
    done < <(jq -r '.diffs[]? | "\(.path): \(.kind) — expected=\(.expected // "null") actual=\(.actual // "null") (\(.severity))"' "$CHECK_JSON" 2>/dev/null | head -20)
    while IFS= read -r line; do
      esc_line="$(escape_wf "$line")"
      echo "::error::$esc_line"
    done < <(jq -r '.warnings[]? | "\(.)"' "$CHECK_JSON" 2>/dev/null | head -10)
  fi

  if [[ $CHECK_EXIT -ne 0 ]]; then
    echo "drift=true" >> "$GITHUB_OUTPUT" 2>/dev/null || true
    if [[ "$FAIL_ON_DRIFT" == "true" ]]; then
      echo "::error::Drift detected — strict check failed"
      exit 1
    else
      echo "::warning::Drift detected but fail-on-drift=false, passing"
      exit 0
    fi
  else
    echo "drift=false" >> "$GITHUB_OUTPUT" 2>/dev/null || true
    echo "✓ No drift — inherited"
    exit 0
  fi

else
  echo "baseline-exists=false" >> "$GITHUB_OUTPUT" 2>/dev/null || true
  echo "baseline-hash=" >> "$GITHUB_OUTPUT" 2>/dev/null || true
  echo "drift=false" >> "$GITHUB_OUTPUT" 2>/dev/null || true
  echo "::notice::No baseline at origin/$BASELINE_REF:$BASELINE_PATH — first-time init, skipping drift check (pass)"
  if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    {
      echo "## Taproot baseline check"
      echo ""
      echo "No baseline found at \`origin/$BASELINE_REF:$BASELINE_PATH\` — first-time init, passing."
      echo "Head hash: \`$HEAD_HASH\`"
    } >> "$GITHUB_STEP_SUMMARY"
  fi
  exit 0
fi
