#!/usr/bin/env python3
"""A node protocol's new-connection time, Go and Rust paired, enough rounds
to tell a few percent apart (#193): not a tier B metric, a closer look at
one. On Linux.

    diag_connect.py --engine go=BIN --engine rust=BIN --fakenode BIN --loadgen BIN
                    --relaytime BIN --engine-cpus L --load-cpus L --node-cpus L
                    [--node anytls] [--rounds 9] [--rate 50] [--seconds 30]
                    [--split-rounds 3]

Two parts, each engine started afresh for each run:

  ROUND   --rounds rounds, the engines in turn and the order swapped every
          round: --rate new connections a second for --seconds, each timed
          from the dial until its first byte comes back; p50 and p99, and
          the p50 of its parts on the client's side (the TCP connection to
          the proxy, the CONNECT's answer). Then SUMMARY: per engine the
          median and quartiles of the rounds' p50s, and the paired
          difference (the second engine's p50 over the first's, per round)
          with its mean and 95% interval.
  SPLIT   --split-rounds rounds with relaytime between the engine and the
          fake node's port, which reads the TLS records: how many TCP
          connections (sessions) the engine opened for its connections, and
          per session, the first and the later ones apart, the medians of:
            hello_us      TCP connection to the ClientHello's arrival
            handshake_us  ClientHello to the client's first record after
                          the server's flight (its Finished): the TLS
                          handshake as the node sees it
            first_app_us  Finished to the next client record (the first
                          application data: the protocol's session)
            node_data_us  that client flight's end to the node's first data
          with the server's first flight (records, bytes: a full handshake
          carries the certificate), the client's second flight (its records
          and their lengths) and whether the ClientHello offers to resume.
"""

import argparse
import json
import math
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import measure as m  # noqa: E402

# Two-sided 95% t quantiles by degrees of freedom.
T95 = {1: 12.706, 2: 4.303, 3: 3.182, 4: 2.776, 5: 2.571, 6: 2.447, 7: 2.365, 8: 2.306, 9: 2.262,
       10: 2.228, 11: 2.201, 12: 2.179, 14: 2.145, 19: 2.093, 29: 2.045}


def t95(df):
    return T95.get(df) or T95[max(k for k in T95 if k <= df)]


def connect_run(args, engine_bin, name, work, ports, seconds, rate):
    engine = m.Engine(engine_bin, work, name, ["--tun=false", "--local-proxy=true"],
                      env=dict(os.environ, SSL_CERT_FILE=ports["certificate"]),
                      wrap=lambda c: m.pinned(args.engine_cpus, c))
    try:
        engine.call("apply-profile", {"profile": m.profile("perf#1", ports)})
        engine.call("start")
        time.sleep(3)
        credential = engine.call("get-local-proxy-credential", {"node_id": args.node})
        return m.loadgen(args, credential, ports, "connect", "-churn-rate", str(rate), "-duration", f"{seconds}s")
    finally:
        engine.stop()


def session(rec):
    """One relayed session's setup, from its TLS records (None if it is not
    a TLS session that reached its first data)."""
    recs = rec.get("records") or []
    if not recs or recs[0]["dir"] != "c" or recs[0]["type"] != 22:
        return None
    i = 1
    while i < len(recs) and recs[i]["dir"] == "c":
        i += 1
    server = []
    while i < len(recs) and recs[i]["dir"] == "s":
        server.append(recs[i])
        i += 1
    client = []
    while i < len(recs) and recs[i]["dir"] == "c":
        client.append(recs[i])
        i += 1
    node = recs[i] if i < len(recs) else None
    # The client's Finished: its first encrypted record (a compatibility
    # ChangeCipherSpec, type 20, may come before it).
    encrypted = [r for r in client if r["type"] == 23]
    if not server or not encrypted or node is None:
        return None
    finished = encrypted[0]
    after = encrypted[1] if len(encrypted) > 1 else None
    return {
        "hello_us": recs[0]["at_ns"] / 1000,
        "handshake_us": (finished["at_ns"] - recs[0]["at_ns"]) / 1000,
        "first_app_us": (after["at_ns"] - finished["at_ns"]) / 1000 if after else None,
        "node_data_us": (node["at_ns"] - client[-1]["at_ns"]) / 1000,
        "server_records": len(server),
        "server_bytes": sum(r["len"] for r in server),
        "client_records": [r["type"] for r in client],
        "client_lengths": [r["len"] for r in client],
        "offers_psk": rec.get("offers_psk") or rec.get("offers_ticket"),
    }


def split(records):
    """The medians of the sessions' setups, the first session apart."""
    records = sorted(records, key=lambda rec: rec["accepted_unix_ns"])
    parts = [s for s in (session(rec) for rec in records) if s]
    out = {"sessions_timed": len(parts)}
    for label, group in (("first", parts[:1]), ("later", parts[1:])):
        if not group:
            continue
        med = lambda k: round(statistics.median(v[k] for v in group if v[k] is not None), 1) if any(v[k] is not None for v in group) else None
        out[label] = {
            "n": len(group),
            "hello_us": med("hello_us"), "handshake_us": med("handshake_us"),
            "first_app_us": med("first_app_us"), "node_data_us": med("node_data_us"),
            "server_records": med("server_records"), "server_bytes": med("server_bytes"),
            "client_records": max(((tuple(v["client_records"]), sum(1 for w in group if w["client_records"] == v["client_records"])) for v in group), key=lambda x: x[1])[0],
            "client_lengths": group[0]["client_lengths"],
            "offers_resume": sum(1 for v in group if v["offers_psk"]),
        }
    return out


def quartiles(values):
    q = statistics.quantiles(values, n=4, method="inclusive") if len(values) > 1 else values * 3
    return q[0], statistics.median(values), q[2]


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--engine", action="append", required=True, metavar="NAME=BIN")
    p.add_argument("--fakenode", required=True)
    p.add_argument("--loadgen", required=True)
    p.add_argument("--relaytime", required=True)
    p.add_argument("--engine-cpus")
    p.add_argument("--load-cpus")
    p.add_argument("--node-cpus")
    p.add_argument("--node", default="anytls", choices=m.PROFILE_NODES)
    p.add_argument("--rounds", type=int, default=9)
    p.add_argument("--rate", type=int, default=50)
    p.add_argument("--seconds", type=int, default=30)
    p.add_argument("--split-rounds", type=int, default=3)
    args = p.parse_args()
    engines = [e.partition("=")[::2] for e in args.engine]
    if len(engines) != 2:
        p.error("two engines: the baseline first")
    work = tempfile.mkdtemp(prefix="diag-connect-")
    node = subprocess.Popen(m.pinned(args.node_cpus, [args.fakenode, "-dir", work]),
                            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    p50s = {name: [] for name, _ in engines}
    try:
        ports = json.loads(node.stdout.readline())
        for rnd in range(1, args.rounds + 1):
            order = engines if rnd % 2 else engines[::-1]
            for name, binary in order:
                r = connect_run(args, binary, f"{name}-{rnd}", work, ports, args.seconds, args.rate)
                p50s[name].append(r["p50_us"])
                print("ROUND " + json.dumps({"engine": name, "round": rnd, "samples": r["samples"], "p50_us": r["p50_us"],
                                             "p99_us": r["p99_us"], "tcp_p50_us": r.get("tcp_p50_us"),
                                             "ok_p50_us": r.get("ok_p50_us")}), flush=True)
        (base, _), (new, _) = engines
        diffs = [n / b - 1 for b, n in zip(p50s[base], p50s[new])]
        mean = statistics.mean(diffs)
        half = t95(len(diffs) - 1) * statistics.stdev(diffs) / math.sqrt(len(diffs)) if len(diffs) > 1 else float("nan")
        summary = {name: dict(zip(("q1_us", "median_us", "q3_us"), quartiles(values))) for name, values in p50s.items()}
        summary["paired"] = {"of": f"{new}/{base}", "rounds": len(diffs), "median_pct": round(100 * statistics.median(diffs), 1),
                             "mean_pct": round(100 * mean, 1), "ci95_pct": [round(100 * (mean - half), 1), round(100 * (mean + half), 1)],
                             "per_round_pct": [round(100 * d, 1) for d in diffs]}
        print("SUMMARY " + json.dumps(summary), flush=True)

        for rnd in range(1, args.split_rounds + 1):
            for name, binary in engines:
                relay = subprocess.Popen(m.pinned(args.node_cpus, [args.relaytime, "-target", f"127.0.0.1:{ports[f'{args.node}_port']}"]),
                                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
                relay_port = json.loads(relay.stdout.readline())["port"]
                records = []
                reader = threading.Thread(target=lambda: records.extend(json.loads(line) for line in relay.stdout))
                reader.start()
                try:
                    r = connect_run(args, binary, f"{name}-split-{rnd}", work,
                                    dict(ports, **{f"{args.node}_port": relay_port}), 20, 20)
                finally:
                    time.sleep(1)
                    relay.stdin.close()
                    relay.terminate()
                    relay.wait(10)
                    reader.join(5)
                print("SPLIT " + json.dumps({"engine": name, "round": rnd, "connections": r["connections"],
                                             "sessions": len(records), "connect_p50_us": r["p50_us"],
                                             **split(records)}), flush=True)
    finally:
        node.stdin.close()
        node.terminate()
        node.wait(10)
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
