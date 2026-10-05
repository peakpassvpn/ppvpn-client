# Test entries, one per tier (docs/testing.md). CI calls these; run them the
# same way by hand. The Go core that lived here is gone: v0.5.21 was its
# last release, and its tag and release files stay.

.PHONY: test-unit test-integration test-system test-tools

# L1: no external dependency. Until the tests are marked by tier (features
# it-sail and system, docs/testing.md), a plain cargo test also runs today's
# in-process sail tests, which are L2.
test-unit:
	cargo test --locked

# L2: a real dependency outside the process, no privileges: today the
# CLI's secret store against the platform's own (Secret Service on Linux,
# the login Keychain on macOS).
test-integration:
	tools/test-integration.sh

# L3: a real TUN, routes, rules and DNS, as root inside network namespaces
# of their own (test/netns/run.sh). Linux only; PART is tun or
# network-change (both when empty). Builds run as the calling user, only
# run.sh under sudo.
test-system:
	tools/test-system-linux.sh $(PART)

# The Go helpers the tests start (module test/go.mod), into build/test-tools.
test-tools:
	mkdir -p build/test-tools
	cd test && CGO_ENABLED=0 go build -trimpath -o ../build/test-tools/fakenode ./fakenode
	cd test && CGO_ENABLED=0 go build -trimpath -o ../build/test-tools/ldnslab ./lab/localdns/ldnslab
