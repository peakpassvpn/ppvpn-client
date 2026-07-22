MOBILE_VERSION := v0.0.0-20260709172247-6129f5bee9d5
GO_BIN := $(shell go env GOPATH)/bin
CLANG_MODULE_CACHE_PATH ?= /tmp/ppvpn-core-clang-cache
export PATH := $(GO_BIN):$(PATH)

.PHONY: test test-race build-desktop build-release-desktop bootstrap-mobile build-mobile-ios build-mobile-android

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

bootstrap-mobile:
	go install golang.org/x/mobile/cmd/gomobile@$(MOBILE_VERSION)
	go install golang.org/x/mobile/cmd/gobind@$(MOBILE_VERSION)
	$(GO_BIN)/gomobile init

build-mobile-ios:
	mkdir -p build
	CLANG_MODULE_CACHE_PATH=$(CLANG_MODULE_CACHE_PATH) $(GO_BIN)/gomobile bind -target=ios -o build/PPVPNCore.xcframework ./mobile

build-mobile-android:
	mkdir -p build
	$(GO_BIN)/gomobile bind -target=android -androidapi 23 -o build/ppvpn-core.aar ./mobile
