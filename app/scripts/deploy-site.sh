#!/bin/zsh
# Deploys the site to production, and refuses to if the new build could not trade.
#
#   1. unit tests, including the comparison of the IDL with the deployed program's
#   2. live check: simulates every instruction against the program on the cluster
#   3. vercel --prod
#   4. waits until the live site answers /api/health from this build, with status ok
#
# Run from anywhere: npm run deploy:site
# ALLOW_BLOCKED=1 deploys while the protocol is paused or capped (the check then
# cannot get past the refused instruction).
set -uo pipefail
cd "$(dirname "$0")/.."
SITE="${SITE_URL:-https://dev.proofofagent.dev}"
# The Vercel CLI does not run on Node 26; Homebrew's Node does.
[ -x /opt/homebrew/bin/npx ] && VERCEL_PATH="/opt/homebrew/bin:$PATH" || VERCEL_PATH="$PATH"

print "1/4 unit tests"
out="$(npm test 2>&1)" || { print -r -- "$out" | grep -E "^✖|not ok|AssertionError|ℹ (pass|fail)" | head -20; print "Unit tests fail: not deploying."; exit 1 }

print "2/4 live check against the deployed program"
npx tsx scripts/check-live.ts
code=$?
if [ $code -eq 2 ] && [ -n "${ALLOW_BLOCKED:-}" ]; then
  print "Continuing because ALLOW_BLOCKED is set."
elif [ $code -ne 0 ]; then
  print "This build could not trade against the deployed program: not deploying."
  exit 1
fi
want="$(npx tsx scripts/check-live.ts --hash)" || exit 1

print "3/4 deploying"
before="$(curl -fsS --max-time 20 "$SITE/api/health" 2>/dev/null | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin).get("idlHash",""))' 2>/dev/null)"
PATH="$VERCEL_PATH" npx vercel --prod --yes 2>&1 | grep -E "Production|Error|error" || { print "vercel failed."; exit 1 }

print "4/4 waiting for $SITE to serve this build"
for i in {1..60}; do
  body="$(curl -fsS --max-time 20 "$SITE/api/health" 2>/dev/null)"
  # 503 means broken: read the body anyway.
  [ -z "$body" ] && body="$(curl -sS --max-time 20 "$SITE/api/health" 2>/dev/null)"
  read -r hash health reason <<< "$(print -r -- "$body" | /usr/bin/python3 -c '
import json, sys
try:
    h = json.load(sys.stdin)
    print(h.get("idlHash", "-"), h.get("status", "-"), h.get("reason", ""))
except Exception:
    print("- - no answer yet")
' 2>/dev/null)"
  # With an unchanged IDL the hash cannot tell the builds apart; vercel has already reported the new one live.
  if [ "$hash" = "$want" ] && [ "$health" = "ok" ]; then
    print "Live and trading: $reason (IDL $hash)"
    exit 0
  fi
  if [ "$hash" = "$want" ] && [ "$health" = "blocked" ] && [ -n "${ALLOW_BLOCKED:-}" ]; then
    print "Live, trading blocked by the program: $reason"
    exit 0
  fi
  if [ "$hash" = "$want" ] && [ "$health" = "broken" ]; then
    break
  fi
  sleep 5
done
print "PROBLEM: the live site reports '$health': $reason (IDL $hash, expected $want; before the deploy it was ${before:-unknown})"
print "Roll back with:  cd app && PATH=\"/opt/homebrew/bin:\$PATH\" npx vercel rollback"
exit 1
