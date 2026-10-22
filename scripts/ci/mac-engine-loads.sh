#!/bin/bash
# Launch the built macOS app headless and prove its AI engine loads under the app's own code signature.
#
#   scripts/ci/mac-engine-loads.sh <path/to/plugable-chat.app> [seconds]
#
# rc15 shipped a .dmg whose app could not load the engine libraries it carries ("different Team IDs": library
# validation under the hardened runtime), so every launch ended at "could not start its AI engine". CI built the
# app and checked its signature and file list but never ran it. This runs it. Two checks:
#   1. the app carries the entitlement that lets it load those libraries;
#   2. a real launch writes the engine's own progress to the test-state file (PLUGABLE_CHAT_TEST_STATE) and does
#      not write an `error` phase. A launch where the engine fails writes `error`/`failed` within a second or two.
# No model is needed: any sign that start-up got past the engine (GPU setup done, a model download or load begun,
# the embedding model starting) is a pass, and the app is stopped straight after.
set -euo pipefail
app="${1:?usage: mac-engine-loads.sh <path/to/App.app> [seconds]}"
limit="${2:-150}"
test -d "$app" || { echo "no such app: $app"; exit 2; }

# 1. entitlement
if ! codesign -d --entitlements - "$app" 2>&1 | grep -q "com.apple.security.cs.disable-library-validation"; then
  echo "FAIL: $app does not carry com.apple.security.cs.disable-library-validation, so macOS will refuse to load the engine libraries inside it (issue 37)."
  exit 1
fi
echo "ok: the app carries the library-validation entitlement"

# 2. launch
exe="$app/Contents/MacOS/$(ls "$app/Contents/MacOS" | head -n 1)"
work="$(mktemp -d)"
state="$work/state.jsonl"
: > "$state"
PLUGABLE_CHAT_HOME="$work/home" PLUGABLE_CHAT_TEST_STATE="$state" "$exe" --hidden --ephemeral > "$work/app.out" 2>&1 &
pid=$!
trap 'kill -9 "$pid" 2>/dev/null || true' EXIT

for ((i = 0; i < limit; i++)); do
  if grep -q '"phase":"error"' "$state"; then
    echo "FAIL: the app reported a start-up error after $i s:"
    grep '"phase":"error"' "$state" | head -n 2 | cut -c1-400
    grep -iE "library load error|code signature|Team ID" "$work/app.out" | head -n 3 | cut -c1-500 || true
    exit 1
  fi
  if grep -qE '"phase":"ep-registration","status":"ok"|"phase":"model-download"|"phase":"ready","status":"ok"|"phase":"embedding"' "$state"; then
    echo "ok: the engine loaded and start-up went on (after $i s)"
    grep -E '"phase":"(ep-registration|model-download|ready|embedding)"' "$state" | head -n 3 | cut -c1-200
    exit 0
  fi
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "FAIL: the app exited before the engine started; last output:"
    tail -n 12 "$work/app.out" | cut -c1-300
    exit 1
  fi
  sleep 1
done
echo "FAIL: no sign of the engine starting within $limit s; last output:"
tail -n 12 "$work/app.out" | cut -c1-300
exit 1
