#!/usr/bin/env bash
# Multi-model daily compact harness (experiment outputs only).
# Usage:
#   ./scripts/compact-day-models.sh YYYY-MM-DD [profiles.env]
#
# profiles.env is sourced; define PROFILES as lines:
#   label|provider|model|base_url_or_empty
#
# Secrets via env only (OPENAI_API_KEY, DEEPSEEK_API_KEY, OPENROUTER_API_KEY).
set -euo pipefail

DAY="${1:?day YYYY-MM-DD required}"
PROFILES_FILE="${2:-}"
MONTH="${DAY:0:7}"
SRC_ROOT="${CASSIO_TRANSCRIPTS:-$HOME/personal/transcripts}"
EXP_ROOT="${CASSIO_EXPERIMENT_ROOT:-$HOME/tmp/cassio-daily-experiment-$DAY}"
CASSIO_BIN="${CASSIO_BIN:-cassio}"

mkdir -p "$EXP_ROOT/input/$MONTH"
rm -f "$EXP_ROOT/input/$MONTH"/${DAY}T*.md
cp "$SRC_ROOT/$MONTH"/${DAY}T*.md "$EXP_ROOT/input/$MONTH/" 2>/dev/null || {
  echo "No session files for $DAY under $SRC_ROOT/$MONTH" >&2
  exit 1
}

MANIFEST="$EXP_ROOT/manifest.json"
echo '[' >"$MANIFEST"
first=1

run_one() {
  local label="$1" provider="$2" model="$3" base_url="${4:-}"
  local out="$EXP_ROOT/out-$label"
  rm -rf "$out"
  mkdir -p "$out"
  local start end elapsed status=0
  start=$(date +%s)
  set +e
  if [[ -n "$base_url" ]]; then
    "$CASSIO_BIN" compact dailies \
      -i "$EXP_ROOT/input" -o "$out" -l 1 \
      -p "$provider" -m "$model" --base-url "$base_url" \
      --chunk-timeout 600 --max-retries 3
    status=$?
  else
    "$CASSIO_BIN" compact dailies \
      -i "$EXP_ROOT/input" -o "$out" -l 1 \
      -p "$provider" -m "$model" \
      --chunk-timeout 600 --max-retries 3
    status=$?
  fi
  set -e
  end=$(date +%s)
  elapsed=$((end - start))
  local daily="$out/$MONTH/${DAY}.daily.md"
  local bytes=0
  [[ -f "$daily" ]] && bytes=$(wc -c <"$daily" | tr -d ' ')
  if [[ $first -eq 0 ]]; then echo ',' >>"$MANIFEST"; fi
  first=0
  cat >>"$MANIFEST" <<EOF
  {
    "label": "$label",
    "provider": "$provider",
    "model": "$model",
    "base_url": "$base_url",
    "exit_code": $status,
    "elapsed_secs": $elapsed,
    "output_bytes": $bytes,
    "daily_path": "$daily"
  }
EOF
}

if [[ -n "$PROFILES_FILE" && -f "$PROFILES_FILE" ]]; then
  # shellcheck disable=SC1090
  source "$PROFILES_FILE"
fi

if [[ -z "${PROFILES:-}" ]]; then
  # Defaults: DeepSeek Pro direct + Luna medium via local codex proxy
  export OPENAI_API_KEY="${OPENAI_API_KEY:-${DEEPSEEK_API_KEY:-proxy}}"
  PROFILES=$(cat <<'EOF'
deepseek-v4-pro|openai|deepseek-v4-pro|https://api.deepseek.com
gpt-5.6-luna-medium|openai|gpt-5.6-luna|http://127.0.0.1:18181/v1
EOF
)
fi

while IFS='|' read -r label provider model base_url; do
  [[ -z "${label// }" || "$label" =~ ^# ]] && continue
  echo "=== $label ($provider / $model) ==="
  if [[ "$model" == deepseek* && -n "${DEEPSEEK_API_KEY:-}" ]]; then
    export OPENAI_API_KEY="$DEEPSEEK_API_KEY"
  fi
  if [[ "$base_url" == *18181* ]]; then
    export OPENAI_API_KEY="${OPENAI_API_KEY:-proxy}"
  fi
  run_one "$label" "$provider" "$model" "$base_url"
done <<<"$PROFILES"

echo ']' >>"$MANIFEST"
echo "Wrote $MANIFEST"
