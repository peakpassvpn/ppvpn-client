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
# get-status with ok and a running state (degraded runs too); else the whole
# group fails here, not check by check (a check whose expectation the host
# meets without a core would pass).
up() {
  status=$( [ -S $R/core.sock ] && api get-status)
  state=$(printf '%s' "$status" | jq -r 'select(.ok == true) | .data.state' 2>/dev/null)
  case $state in
  running|degraded) echo "== core up (engine=$ENGINE, state=$state)"; return 0 ;;
  esac
  echo "FAIL $1.up the core runs and answers get-status: the group is not run"
  echo "  get-status: $(printf '%s' "$status" | head -c 600)"
  [ -f $R/stdout ] && sed 's/^/  stdout: /' $R/stdout | tail -20
  [ -f $R/core.log ] && sed 's/^/  log: /' $R/core.log | tail -20
  CASE_FAIL=$((CASE_FAIL + 1)); summary; exit 1
}
# dials_taken <group>: no dial of the run was refused as not running (sail
# takes dials once its outbounds are built, before it resolves names: an
# early dns-local query refused so is a regression), with at least one dns
# line logged, so that an empty log does not pass. Run before stop_core: a
# dial refused while stopping is not one.
dials_taken() {
  dns=$(grep -c 'msg=dns ' $R/core.log 2>/dev/null)
  refused=$(grep -c 'not_running' $R/core.log 2>/dev/null)
  check "$1.start" "dials while starting are taken: refused not_running, dns lines" \
    "refused=${refused:-0} dns_lines=$([ "${dns:-0}" -gt 0 ] && echo some || echo none)" 'refused=0 dns_lines=some'
}
# outbound_to <destination>: the outbound of the last connection log line to
# destination (host:port).
outbound_to() { grep -E "msg=connection .*destination=$1( |$)" $R/core.log | sed -n 's/.*outbound=\([^ ]*\).*/\1/p' | tail -1; }
summary() {
  echo "== $CASE_PASS passed, $CASE_FAIL failed (engine=$ENGINE)"
  # On a failure, the log lines the checks read (dns, connection, kernel
  # switched), so that a missing or differently written line shows here: a
  # reported-only step uploads no artifact.
  if [ "$CASE_FAIL" != 0 ] && [ -f $R/core.log ]; then
    grep -E 'msg=(dns|connection|"kernel switched"|"dns:)' $R/core.log | tail -15 | sed 's/^/  log: /'
    echo "  log: $(wc -l < $R/core.log) lines, $(grep -c 'level=debug' $R/core.log) at debug"
  fi
  [ "$CASE_FAIL" = 0 ]
}
# exit_of: the web target's "exit=<address> host=<host>" -> the address.
exit_of() { sed -n 's/.*exit=\([^ ]*\).*/\1/p'; }
