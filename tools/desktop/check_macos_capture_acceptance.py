#!/usr/bin/env python3
"""Validate isolated macOS capture evidence, without changing network state."""
import argparse
import ipaddress
import json
import re
from pathlib import Path


def require(condition, message):
    if not condition:
        raise ValueError(message)


def dns_state(text):
    # Reachability changes independently of DNS ownership. Keep server, scope,
    # search domain, flags and order; ignore only the reachability observation.
    blocks, block, section = [], [], ""
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("DNS configuration"):
            if block:
                blocks.append((section, tuple(sorted(block))))
                block = []
            section = line
        elif re.fullmatch(r"resolver #\d+", line):
            if block:
                blocks.append((section, tuple(sorted(block))))
                block = []
        elif line and not re.match(r"reach\s*:", line):
            block.append(line)
    if block:
        blocks.append((section, tuple(sorted(block))))
    require(blocks, "missing DNS snapshot")
    return sorted(blocks)


def route_network(value, ipv6):
    if value == "default":
        return ipaddress.ip_network("::/0" if ipv6 else "0.0.0.0/0")
    value = value.split("%", 1)[0] if "/" not in value else value
    if not ipv6:
        address, separator, prefix = value.partition("/")
        if not re.fullmatch(r"\d+(?:\.\d+){0,3}", address):
            return None
        address = ".".join(address.split(".") + ["0"] * (4 - len(address.split("."))))
        value = address + (separator + prefix if separator else "/32")
    try:
        return ipaddress.ip_network(value, strict=False)
    except ValueError:
        return None


def capture_routes(text):
    require("Destination" in text and "Internet:" in text, "missing route table")
    rows, ipv6, interface_column = [], False, None
    split_routes = {ipaddress.ip_network(x) for x in
                    ("0.0.0.0/1", "128.0.0.0/1", "::/1", "8000::/1")}
    owned = [ipaddress.ip_network("10.60.159.88/30"),
             ipaddress.ip_network("fde2:ec40:9312:c7fd::/126")]
    for line in text.splitlines():
        fields = line.split()
        if line.startswith("Internet"):
            ipv6 = line.startswith("Internet6")
            interface_column = None
        elif fields and fields[0] == "Destination":
            require("Netif" in fields, "missing route interface column")
            interface_column = fields.index("Netif")
        elif interface_column is not None and len(fields) > interface_column:
            network = route_network(fields[0], ipv6)
            if network is None:
                continue
            interface = fields[interface_column]
            relevant = (network in split_routes or interface.startswith("utun") or
                        any(network.version == subnet.version and
                            network.subnet_of(subnet) for subnet in owned))
            if relevant:
                # Expiration and packet counters are not route ownership.
                rows.append((str(network), fields[1], fields[2], interface))
    return sorted(rows)


def default_route(text):
    values = {}
    for line in text.splitlines():
        key, separator, value = line.strip().partition(":")
        if separator and key in {"gateway", "interface", "destination", "mask"}:
            values[key] = value.strip()
    require("gateway" in values and "interface" in values, "missing default route")
    return values


def validate(directory):
    rows = [json.loads(line) for line in
            (directory / "native-probes.jsonl").read_text().splitlines()
            if line.startswith("{")]
    require(rows and rows[-1].get("probe") == "complete", "native probe incomplete")
    expected = list(range(20))
    for phase in ("attached", "detached"):
        events = [row for row in rows if row.get("probe") == phase]
        require([row.get("cycle") for row in events] == expected,
                f"{phase}: twenty distinct ordered cycles required")
    events = [row for row in rows if row.get("probe") in {"attached", "detached"}]
    require([row["probe"] for row in events] == ["attached", "detached"] * 20,
            "capture transitions are not paired")
    owner, runtime = events[0].get("owner"), events[0].get("runtime_ids")
    require(owner is not None and isinstance(runtime, list) and len(runtime) == 1,
            "missing unique forwarding owner")
    require(all(row.get("owner") == owner and row.get("runtime_ids") == runtime
                for row in events), "forwarding owner/runtime changed")
    complete = rows[-1]
    require(complete.get("cycles") == 20 and complete.get("runtime_ids") == [],
            "runtime did not settle after twenty cycles")
    for key in ("http_connect", "socks_tcp", "socks_udp", "per_node_http_connect",
                "per_node_socks_tcp", "per_node_socks_udp"):
        require(complete.get(key) is True, f"missing actual {key} continuity result")
    baseline = {}
    for stage in ("before", "detached", "after"):
        dns_text = (directory / f"dns-{stage}.txt").read_text()
        require("com.peakpassvpn.capture." not in dns_text and
                "10.60.159.90" not in dns_text, f"capture DNS remains {stage}")
        state = {
            "dns": dns_state(dns_text),
            "routes": capture_routes((directory / f"routes-{stage}.txt").read_text()),
            "default": default_route((directory / f"default-{stage}.txt").read_text()),
            "utun_interfaces": sorted(name for name in
                (directory / f"interfaces-{stage}.txt").read_text().split()
                if re.fullmatch(r"utun\d+", name)),
        }
        if stage == "before":
            baseline = state
        else:
            for key, value in state.items():
                require(value == baseline[key], f"{key} not restored {stage}")
    return {"cycles": 20, "routed_and_per_node_tcp_udp": True,
            "owner_unchanged": True, "dns_and_capture_routes_restored": True,
            "utun_interfaces_restored": True}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("evidence", type=Path)
    args = parser.parse_args()
    try:
        result = validate(args.evidence)
    except (ValueError, KeyError, OSError) as error:
        raise SystemExit(f"native acceptance failed: {error}") from error
    (args.evidence / "verified-native-acceptance.json").write_text(
        json.dumps(result, indent=2) + "\n")
