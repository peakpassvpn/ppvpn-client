# Sourced by the case scripts (cases/<group>.sh), inside $LAB-client. A case
# script starts the engine through core.sh (ENGINE: sing = Go core; rust =
# ppvpn-core-lab), runs its checks and prints one line per
# check; the same script and expectations run on every engine.
#
#   check <id> <what> <got> <want-ERE>     PASS when got matches ^(want)$
#   summary                                 prints the totals; exit 1 on a FAIL
#
# Output lines: "PASS <id> <what>: got=<got>" or
# "FAIL <id> <what>: got=<got> want=<want>". Go core 0.5.21's output is kept
# as cases/<group>.baseline.txt for reference.
. /lab/core.sh
CASE_PASS=0; CASE_FAIL=0
check() {
  if printf '%s' "$3" | grep -qxE "$4"; then CASE_PASS=$((CASE_PASS + 1)); echo "PASS $1 $2: got=$3"
  else CASE_FAIL=$((CASE_FAIL + 1)); echo "FAIL $1 $2: got=$3 want=$4"; fi
}
# up <group>: the core started, applied, started its run and answers
# get-status with ok; else the whole group fails here, not check by check
# (a check whose expectation the host meets without a core would pass).
up() {
  if [ -S $R/core.sock ] && api get-status | jq -e '.ok == true and .data.state == "running"' >/dev/null 2>&1; then
    echo "== core up (engine=$ENGINE)"; return 0
  fi
  echo "FAIL $1.up the core runs and answers get-status: the group is not run"
  [ -f $R/stdout ] && sed 's/^/  stdout: /' $R/stdout | tail -20
  [ -f $R/core.log ] && sed 's/^/  log: /' $R/core.log | tail -20
  CASE_FAIL=$((CASE_FAIL + 1)); summary; exit 1
}
# outbound_to <destination>: the outbound of the last connection log line to
# destination (host:port).
outbound_to() { grep -E "msg=connection .*destination=$1( |$)" $R/core.log | sed -n 's/.*outbound=\([^ ]*\).*/\1/p' | tail -1; }
summary() { echo "== $CASE_PASS passed, $CASE_FAIL failed (engine=$ENGINE)"; [ "$CASE_FAIL" = 0 ]; }
# exit_of: the web target's "exit=<address> host=<host>" -> the address.
exit_of() { sed -n 's/.*exit=\([^ ]*\).*/\1/p'; }
