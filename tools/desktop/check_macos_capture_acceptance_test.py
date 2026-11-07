#!/usr/bin/env python3
"""Negative evidence checks: a passing default route must not hide capture leaks."""
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "capture_check", Path(__file__).with_name("check_macos_capture_acceptance.py"))
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)

DNS = """DNS configuration

resolver #1
  nameserver[0] : 192.0.2.53
  if_index : 4 (en0)
  flags : Request A records
  reach : 0x00000002 (Reachable)
  order : 200000
"""
ROUTES = """Routing tables

Internet:
Destination        Gateway            Flags        Netif Expire
default            192.0.2.1          UGScg          en0
127                127.0.0.1          UCS            lo0

Internet6:
Destination        Gateway            Flags        Netif Expire
default            fe80::1%en0        UGcIg          en0
"""
DEFAULT = """route to: default
destination: default
mask: default
gateway: 192.0.2.1
interface: en0
"""


class AcceptanceEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ppvpn-evidence-check-")
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.rows = []
        for cycle in range(20):
            for phase in ("attached", "detached"):
                self.rows.append({"probe": phase, "cycle": cycle,
                                  "owner": {"instance": "one-owner", "generation": 1}, "runtime_ids": [42]})
        self.rows.append({"probe": "complete", "cycles": 20, "runtime_ids": [],
                          "http_connect": True, "socks_tcp": True, "socks_udp": True,
                          "per_node_tcp": True, "per_node_upstream_connections": 1,
                          "per_node_sink_accepts": 1, "routed_udp_upstream_peers": 1})
        self.write_events()
        for stage in ("before", "detached", "after"):
            for kind, value in (("dns", DNS), ("routes", ROUTES),
                                ("default", DEFAULT), ("interfaces", "lo0 en0\n")):
                (self.directory / f"{kind}-{stage}.txt").write_text(value)

    def write_events(self):
        (self.directory / "native-probes.jsonl").write_text(
            "\n".join(json.dumps(row) for row in self.rows) + "\n")

    def reject(self, expected):
        with self.assertRaisesRegex(ValueError, expected):
            check.validate(self.directory)

    def test_complete_evidence_accepts_reachability_only_change(self):
        (self.directory / "dns-after.txt").write_text(
            DNS.replace("0x00000002 (Reachable)", "0x00000000 (Not Reachable)"))
        self.assertEqual(check.validate(self.directory)["cycles"], 20)

    def test_twenty_events_cannot_repeat_one_cycle(self):
        self.rows[12]["cycle"] = 0
        self.write_events()
        self.reject("distinct ordered cycles")

    def test_restart_cannot_masquerade_as_continuity(self):
        self.rows[10]["runtime_ids"] = [43]
        self.write_events()
        self.reject("owner/runtime changed")

    def test_same_owner_with_second_runtime_is_rejected(self):
        self.rows[17]["runtime_ids"] = [42, 43]
        self.write_events()
        self.reject("owner/runtime changed")

    def test_routed_only_probe_is_insufficient(self):
        del self.rows[-1]["per_node_tcp"]
        self.write_events()
        self.reject("per_node_tcp")

    def test_reconnected_upstream_is_not_continuity(self):
        self.rows[-1]["per_node_upstream_connections"] = 2
        self.write_events()
        self.reject("exactly one established connection")

    def test_boolean_cannot_replace_measured_connection_count(self):
        self.rows[-1]["per_node_sink_accepts"] = True
        self.write_events()
        self.reject("exactly one established connection")

    def test_udp_reconnection_or_unmeasured_count_is_rejected(self):
        for value in [None, 0, 2, True]:
            with self.subTest(value=value):
                self.rows[-1]["routed_udp_upstream_peers"] = value
                self.write_events()
                self.reject("routed UDP upstream")

    def test_dns_server_change_is_not_reachability_noise(self):
        (self.directory / "dns-detached.txt").write_text(
            DNS.replace("192.0.2.53", "192.0.2.54"))
        self.reject("dns not restored detached")

    def test_capture_dns_leftover_is_rejected(self):
        (self.directory / "dns-after.txt").write_text(
            DNS.replace("192.0.2.53", "10.60.159.90"))
        self.reject("capture DNS remains after")

    def test_split_route_leak_fails_even_with_original_default(self):
        (self.directory / "routes-detached.txt").write_text(ROUTES.replace(
            "127                127.0.0.1          UCS            lo0",
            "0/1                link#19            UCS          utun7\n"
            "128.0/1            link#19            UCS          utun7"))
        self.reject("routes not restored detached")

    def test_interface_leak_fails_without_route_leak(self):
        (self.directory / "interfaces-after.txt").write_text("lo0 en0 utun7\n")
        self.reject("utun_interfaces not restored after")

    def test_missing_table_is_not_empty_success(self):
        (self.directory / "routes-after.txt").write_text("")
        self.reject("missing route table")

    def test_incomplete_probes_are_rejected(self):
        self.rows.pop()
        self.write_events()
        self.reject("native probe incomplete")


if __name__ == "__main__":
    unittest.main()
