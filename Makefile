MOBILE_VERSION := v0.0.0-20260709172247-6129f5bee9d5
GO_BIN := $(shell go env GOPATH)/bin
CLANG_MODULE_CACHE_PATH ?= /tmp/ppvpn-core-clang-cache
export PATH := $(GO_BIN):$(PATH)

.PHONY: test test-race build-desktop build-release-desktop build-desktop-artifact bootstrap-mobile build-mobile-ios build-mobile-macos build-mobile-android verify-mobile-macos

test:
	go test ./...

test-race:
	go test -race ./...

build-desktop:
	go build -trimpath -o build/ppvpn-core ./cmd/ppvpn-core

build-release-desktop:
	mkdir -p build
	GOOS=darwin GOARCH=arm64 go build -trimpath -o build/ppvpn-core-darwin-arm64 ./cmd/ppvpn-core
	GOOS=darwin GOARCH=amd64 go build -trimpath -o build/ppvpn-core-darwin-amd64 ./cmd/ppvpn-core
	GOOS=windows GOARCH=amd64 go build -trimpath -o build/ppvpn-core-windows-amd64.exe ./cmd/ppvpn-core

build-desktop-artifact: build-mobile-macos
	mkdir -p build
	CGO_ENABLED=0 GOOS=windows GOARCH=amd64 go build -trimpath -o build/ppvpn-core-windows-amd64.exe ./cmd/ppvpn-core
	file build/ppvpn-core-windows-amd64.exe | grep -q 'PE32+'
	shasum -a 256 build/ppvpn-core-windows-amd64.exe > build/desktop-SHA256SUMS
	find build/PPVPNCore.xcframework -type f -print0 | sort -z | xargs -0 shasum -a 256 >> build/desktop-SHA256SUMS

bootstrap-mobile:
	go install golang.org/x/mobile/cmd/gomobile@$(MOBILE_VERSION)
	go install golang.org/x/mobile/cmd/gobind@$(MOBILE_VERSION)
	$(GO_BIN)/gomobile init

build-mobile-ios:
	mkdir -p build
	CLANG_MODULE_CACHE_PATH=$(CLANG_MODULE_CACHE_PATH) $(GO_BIN)/gomobile bind -target=ios -o build/PPVPNCore.xcframework ./mobile

build-mobile-macos:
	mkdir -p build
	scripts/build-macos-xcframework.sh

verify-mobile-macos:
	scripts/verify-macos-xcframework.sh build/PPVPNCore.xcframework

build-mobile-android:
	mkdir -p build
	$(GO_BIN)/gomobile bind -target=android -androidapi 23 -o build/ppvpn-core.aar ./mobile
