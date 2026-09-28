#!/bin/zsh
# Local uptime check for Proof of Agent. Run every 5 minutes by launchd
# (dev.proofofagent.uptime) from ~/Library/Application Support/ProofOfAgent,
# because macOS blocks background jobs from reading the Desktop.
#
# Checks the agent runner heartbeat, both sites, and the faucet balance.
# Logs every failing check and every recovery to uptime.log, and shows a
# macOS notification when the overall state changes. Nothing is logged while
# all is well, apart from one "ok" line a day so you can see it is running.
# Only runs while the Mac is awake.
set -uo pipefail

DIR="${UPTIME_DIR:-$HOME/Library/Application Support/ProofOfAgent}"
LOG="$DIR/uptime.log"
STATE="$DIR/uptime.state"
AGENTS_EXPECTED=${UPTIME_AGENTS:-3}
FAUCET_LOW=${UPTIME_FAUCET_LOW:-5}

mkdir -p "$DIR"
now() { date -u +%Y-%m-%dT%H:%M:%SZ; }
log() { print -r -- "$(now) $*" >> "$LOG"; }
notify() { osascript -e "display notification \"$2\" with title \"Proof of Agent\" subtitle \"$1\"" >/dev/null 2>&1 || true; }
get() { curl -fsS --max-time 20 --retry 2 --retry-delay 3 "$@" 2>/dev/null; }

# If this Mac is offline, say so once and skip: that is not the service's fault.
if ! curl -fsS --max-time 10 -o /dev/null https://www.cloudflare.com/cdn-cgi/trace 2>/dev/null; then
  [ "$(cat "$STATE" 2>/dev/null)" = "offline-local" ] || log "skipped: this Mac has no internet"
  print -r -- "offline-local" > "$STATE"
  exit 0
fi

problems=()

beat="$(get https://dev.proofofagent.dev/api/heartbeat)"
if [ -z "$beat" ]; then
  problems+=("heartbeat endpoint unreachable")
else
  summary="$(print -r -- "$beat" | /usr/bin/python3 -c '
import json, sys
b = json.load(sys.stdin)
print("online" if b.get("online") else "offline", b.get("ageSec"), len(b.get("agents") or []))
' 2>/dev/null)"
  read -r online age agents <<< "$summary"
  if [ "$online" != "online" ]; then
    problems+=("runner offline (last heartbeat ${age:-never}s ago)")
  elif [ "${agents:-0}" -lt "$AGENTS_EXPECTED" ]; then
    problems+=("runner reports $agents of $AGENTS_EXPECTED agents")
  fi
fi

for url in https://dev.proofofagent.dev/ https://proofofagent.dev/; do
  code="$(curl -s -o /dev/null --max-time 20 -w '%{http_code}' "$url")"
  [ "$code" = "200" ] || problems+=("$url returned $code")
done

drops="$(get https://dev.proofofagent.dev/api/faucet | /usr/bin/python3 -c '
import json, sys
f = json.load(sys.stdin)
print(f.get("remainingDrops", 0) if f.get("enabled") else "off")
' 2>/dev/null)"
if [ "$drops" = "off" ] || [ -z "$drops" ]; then
  problems+=("faucet is off or unreachable")
elif [ "$drops" -le "$FAUCET_LOW" ]; then
  problems+=("faucet low: $drops payouts left, top up CXBGKyrkwWFjVaEGgAinSFcbSKnfcuoUcZ3xQjCiTm59")
fi

prev="$(cat "$STATE" 2>/dev/null)"
if [ ${#problems[@]} -eq 0 ]; then
  if [ -n "$prev" ] && [ "$prev" != "ok" ] && [ "$prev" != "offline-local" ]; then
    log "RECOVERED: all checks pass"
    notify "Recovered" "All checks pass"
  elif ! grep -q "^$(date -u +%Y-%m-%d).* ok" "$LOG" 2>/dev/null; then
    log "ok (daily check-in)"
  fi
  print -r -- "ok" > "$STATE"
else
  joined="${(j:; :)problems}"
  log "PROBLEM: $joined"
  [ "$prev" = "$joined" ] || notify "Problem" "$joined"
  print -r -- "$joined" > "$STATE"
fi

# Keep the log small.
if [ "$(wc -l < "$LOG")" -gt 5000 ]; then
  tail -n 2000 "$LOG" > "$LOG.tmp" && mv "$LOG.tmp" "$LOG"
fi
