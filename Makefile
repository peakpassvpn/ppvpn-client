MOBILE_VERSION := v0.0.0-20260709172247-6129f5bee9d5
GO_BIN := $(shell go env GOPATH)/bin
CLANG_MODULE_CACHE_PATH ?= /tmp/ppvpn-core-clang-cache
# VLESS REALITY needs sing-box's uTLS support, which is behind a build tag.
GO_TAGS ?= with_utls
export GO_TAGS
# Desktop CLI cores also run sing-box TUN (`serve --tun`). The default and
# service-selected TUN stack "mixed" (and "gvisor") only exist with the
# with_gvisor tag; without it TUN start fails with "gVisor is not included".
# with_dhcp lets sing-box's local DNS transport on Darwin query the resolvers
# DHCP advertised while a TUN exists, instead of falling back to the system
# resolver (which the desktop points at the tunnel). Hosts should still pass
# --local-dns-servers; DHCP is the fallback.
# Mobile builds keep GO_TAGS: they use the platform tunnel, not this stack.
DESKTOP_TAGS ?= $(GO_TAGS),with_gvisor,with_dhcp
export PATH := $(GO_BIN):$(PATH)
# Windows version resource (Properties > Details): FileVersion and
# ProductVersion come from CoreVersion, the one place the version is
# written (version/version.go); the rest is winres/winres.json. The .syso
# files are generated for the build and removed after it, never committed.
GO_WINRES := go run github.com/tc-hib/go-winres@v0.3.3
CORE_VERSION := $(shell sed -n 's/^[[:space:]]*CoreVersion[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' version/version.go)
WINDOWS_VERSION := $(CORE_VERSION).0

.PHONY: test test-race print-desktop-tags windows-resources verify-windows-version build-lab-linux build-desktop build-release-desktop build-desktop-artifact build-macos-artifact build-macos-cli-artifact build-windows-artifact build-linux-artifact bootstrap-mobile build-mobile-ios build-mobile-macos build-mobile-android build-ios-artifact build-android-artifact verify-mobile-ios verify-mobile-macos verify-mobile-android

test:
	go test -tags $(GO_TAGS) ./...

test-race:
	go test -tags $(GO_TAGS) -race ./...

# CI builds with exactly the release tags.
print-desktop-tags:
	@echo $(DESKTOP_TAGS)

# Lab build for proto/sail-lab tests of dns-local (reads its servers from
# $$PPVPN_LOCALDNS_TEST_FILE); never shipped: release.yml rejects it.
build-lab-linux:
	CGO_ENABLED=0 GOOS=linux GOARCH=$(or $(GOARCH),amd64) go build -tags $(DESKTOP_TAGS),localdns_testsource -trimpath -o build/ppvpn-core-lab-linux ./cmd/ppvpn-core

build-desktop:
	go build -tags $(DESKTOP_TAGS) -trimpath -o build/ppvpn-core ./cmd/ppvpn-core

# Generates cmd/ppvpn-core/rsrc_windows_{amd64,arm64}.syso; go build links
# them into Windows builds of that architecture only.
windows-resources:
	test -n "$(CORE_VERSION)"
	$(GO_WINRES) make --in winres/winres.json --arch amd64,arm64 --out cmd/ppvpn-core/rsrc --file-version $(WINDOWS_VERSION) --product-version $(WINDOWS_VERSION)

# Reads the version resource back from both Windows binaries and fails
# unless every version field is CoreVersion's.
verify-windows-version:
	set -e; for arch in amd64 arm64; do \
		dir=$$(mktemp -d); \
		$(GO_WINRES) extract --dir $$dir build/ppvpn-core-windows-$$arch.exe; \
		for key in file_version product_version FileVersion ProductVersion; do \
			grep -q "\"$$key\": \"$(WINDOWS_VERSION)\"" $$dir/winres.json || { echo "ppvpn-core-windows-$$arch.exe: $$key is not $(WINDOWS_VERSION)" >&2; cat $$dir/winres.json >&2; exit 1; }; \
		done; \
		grep -q '"CompanyName": "PeakPass VPN LLC"' $$dir/winres.json || { echo "ppvpn-core-windows-$$arch.exe: no CompanyName" >&2; exit 1; }; \
		rm -rf $$dir; \
	done

build-release-desktop: windows-resources
	mkdir -p build
	GOOS=darwin GOARCH=arm64 go build -tags $(DESKTOP_TAGS) -trimpath -o build/ppvpn-core-darwin-arm64 ./cmd/ppvpn-core
	GOOS=darwin GOARCH=amd64 go build -tags $(DESKTOP_TAGS) -trimpath -o build/ppvpn-core-darwin-amd64 ./cmd/ppvpn-core
	GOOS=windows GOARCH=amd64 go build -tags $(DESKTOP_TAGS) -trimpath -o build/ppvpn-core-windows-amd64.exe ./cmd/ppvpn-core
	GOOS=linux GOARCH=amd64 go build -tags $(DESKTOP_TAGS) -trimpath -o build/ppvpn-core-linux-amd64 ./cmd/ppvpn-core
	rm -f cmd/ppvpn-core/rsrc_windows_*.syso

build-macos-artifact: build-mobile-macos
	mkdir -p build
	rm -f build/PPVPNCore-macos.xcframework.zip build/macos-SHA256SUMS
	ditto -c -k --sequesterRsrc --keepParent build/PPVPNCore.xcframework build/PPVPNCore-macos.xcframework.zip
	shasum -a 256 build/PPVPNCore-macos.xcframework.zip > build/macos-SHA256SUMS

build-macos-cli-artifact:
	mkdir -p build
	CGO_ENABLED=0 GOOS=darwin GOARCH=arm64 go build -tags $(DESKTOP_TAGS) -trimpath -o build/ppvpn-core-darwin-arm64 ./cmd/ppvpn-core
	CGO_ENABLED=0 GOOS=darwin GOARCH=amd64 go build -tags $(DESKTOP_TAGS) -trimpath -o build/ppvpn-core-darwin-amd64 ./cmd/ppvpn-core
	lipo build/ppvpn-core-darwin-arm64 -verify_arch arm64
	lipo build/ppvpn-core-darwin-amd64 -verify_arch x86_64
	shasum -a 256 build/ppvpn-core-darwin-arm64 build/ppvpn-core-darwin-amd64 > build/macos-cli-SHA256SUMS

build-windows-artifact: windows-resources
	mkdir -p build
	CGO_ENABLED=0 GOOS=windows GOARCH=amd64 go build -tags $(DESKTOP_TAGS) -trimpath -o build/ppvpn-core-windows-amd64.exe ./cmd/ppvpn-core
	CGO_ENABLED=0 GOOS=windows GOARCH=arm64 go build -tags $(DESKTOP_TAGS) -trimpath -o build/ppvpn-core-windows-arm64.exe ./cmd/ppvpn-core
	rm -f cmd/ppvpn-core/rsrc_windows_*.syso
	go version -m build/ppvpn-core-windows-amd64.exe | grep -Eq 'build[[:space:]]+GOOS=windows'
	go version -m build/ppvpn-core-windows-amd64.exe | grep -Eq 'build[[:space:]]+GOARCH=amd64'
	go version -m build/ppvpn-core-windows-arm64.exe | grep -Eq 'build[[:space:]]+GOOS=windows'
	go version -m build/ppvpn-core-windows-arm64.exe | grep -Eq 'build[[:space:]]+GOARCH=arm64'
	go version -m build/ppvpn-core-windows-amd64.exe | grep -Eq 'build[[:space:]]+-tags=.*with_gvisor'
	go version -m build/ppvpn-core-windows-arm64.exe | grep -Eq 'build[[:space:]]+-tags=.*with_gvisor'
	shasum -a 256 build/ppvpn-core-windows-amd64.exe build/ppvpn-core-windows-arm64.exe > build/windows-SHA256SUMS

build-linux-artifact:
	mkdir -p build
	CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -tags $(DESKTOP_TAGS) -trimpath -o build/ppvpn-core-linux-amd64 ./cmd/ppvpn-core
	CGO_ENABLED=0 GOOS=linux GOARCH=arm64 go build -tags $(DESKTOP_TAGS) -trimpath -o build/ppvpn-core-linux-arm64 ./cmd/ppvpn-core
	go version -m build/ppvpn-core-linux-amd64 | grep -Eq 'build[[:space:]]+GOOS=linux'
	go version -m build/ppvpn-core-linux-amd64 | grep -Eq 'build[[:space:]]+GOARCH=amd64'
	go version -m build/ppvpn-core-linux-arm64 | grep -Eq 'build[[:space:]]+GOOS=linux'
	go version -m build/ppvpn-core-linux-arm64 | grep -Eq 'build[[:space:]]+GOARCH=arm64'
	shasum -a 256 build/ppvpn-core-linux-amd64 build/ppvpn-core-linux-arm64 > build/linux-SHA256SUMS

build-desktop-artifact: build-macos-artifact build-macos-cli-artifact build-windows-artifact build-linux-artifact

bootstrap-mobile:
	go install golang.org/x/mobile/cmd/gomobile@$(MOBILE_VERSION)
	go install golang.org/x/mobile/cmd/gobind@$(MOBILE_VERSION)

build-mobile-ios:
	mkdir -p build
	CLANG_MODULE_CACHE_PATH=$(CLANG_MODULE_CACHE_PATH) $(GO_BIN)/gomobile bind -tags $(GO_TAGS) -target=ios -o build/PPVPNCore.xcframework ./mobile

verify-mobile-ios:
	scripts/verify-ios-xcframework.sh build/PPVPNCore.xcframework

build-ios-artifact: build-mobile-ios verify-mobile-ios
	rm -f build/PPVPNCore-ios.xcframework.zip build/ios-SHA256SUMS
	ditto -c -k --sequesterRsrc --keepParent build/PPVPNCore.xcframework build/PPVPNCore-ios.xcframework.zip
	shasum -a 256 build/PPVPNCore-ios.xcframework.zip > build/ios-SHA256SUMS

build-mobile-macos:
	mkdir -p build
	scripts/build-macos-xcframework.sh

verify-mobile-macos:
	scripts/verify-macos-xcframework.sh build/PPVPNCore.xcframework

build-mobile-android:
	mkdir -p build
	$(GO_BIN)/gomobile bind -tags $(GO_TAGS) -target=android -androidapi 23 -o build/ppvpn-core.aar ./mobile

verify-mobile-android:
	scripts/verify-android-aar.sh build/ppvpn-core.aar

build-android-artifact: build-mobile-android verify-mobile-android
	rm -f build/android-SHA256SUMS
	shasum -a 256 build/ppvpn-core.aar > build/android-SHA256SUMS
