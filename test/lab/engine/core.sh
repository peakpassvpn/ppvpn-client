# Sourced inside lab-client: helpers to run the core and call its API.
R=/run/core
start_core() {
  export PPVPN_SAIL_FAILOVER=${SAIL_FAILOVER:-} # engine (sail|sing), then extra serve flags
  engine=$1; shift
  # sing, sail: the Go core (ppvpn-core, its engine by PPVPN_ENGINE); rust:
  # the Rust engine through ppvpn-core-lab (lab.sh up ... <ppvpn-core-lab>),
  # same flags, log lines and Core API v1.
  case $engine in
  rust) bin=${RUST_CORE_BIN:-/work/ppvpn-core-lab} ;;
  *) bin=${CORE_BIN:-ppvpn-core} ;;
  esac
  stop_core; rm -rf $R; mkdir -p $R
  PPVPN_ENGINE=$engine PPVPN_SAIL_BINARY=${SAIL_BIN:-/sail/sail} PPVPN_SAIL_LOG=${SAIL_LOG:-debug} \
  PPVPN_SAIL_CHECK_URL=${CHECK_URL:-http://198.51.100.50/generate_204} PPVPN_SAIL_CHECK_INTERVAL=${CHECK_INTERVAL:-} PPVPN_SAIL_CHECK_TIMEOUT=${CHECK_TIMEOUT:-} \
    $bin serve --socket $R/core.sock --session-secret-file $R/secret --state-dir $R/state \
    --log-file $R/core.log --log-level ${LOG_LEVEL:-debug} "$@" >$R/stdout 2>&1 &
  echo $! > $R/pid
  for i in $(seq 50); do [ -S $R/core.sock ] && [ -s $R/secret ] && return 0; sleep 0.1; done
  echo "core did not start"; cat $R/stdout $R/core.log; return 1
}
stop_core() { [ -f $R/pid ] && kill $(cat $R/pid) 2>/dev/null && sleep 0.5; pkill -x sail 2>/dev/null; return 0; }
api() { # method [json body]
  curl -s --unix-socket $R/core.sock -H "Authorization: Bearer $(cat $R/secret)" -X POST "http://core/v1/$1" -d "${2:-{\}}"
}
apply() { # [routing_mode]
  jq -c --arg mode "${1:-rules}" '{profile: ., routing_mode: $mode}' ${PROFILE:-/work/profile.json} > $R/apply.json
  curl -s --unix-socket $R/core.sock -H "Authorization: Bearer $(cat $R/secret)" -X POST http://core/v1/apply-profile -d @$R/apply.json
}
cred() { api get-local-proxy-credential "$1" | jq -r '.data | "\(.username):\(.password)@\(.listen):\(.port)"'; }
