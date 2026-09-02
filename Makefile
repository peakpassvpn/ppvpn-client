MOBILE_VERSION := v0.0.0-20260709172247-6129f5bee9d5
GO_BIN := $(shell go env GOPATH)/bin
CLANG_MODULE_CACHE_PATH ?= /tmp/ppvpn-core-clang-cache
export PATH := $(GO_BIN):$(PATH)

.PHONY: test test-race build-desktop build-release-desktop build-desktop-artifact build-macos-artifact build-windows-artifact bootstrap-mobile build-mobile-ios build-mobile-macos build-mobile-android build-ios-artifact build-android-artifact verify-mobile-ios verify-mobile-macos verify-mobile-android

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

build-macos-artifact: build-mobile-macos
	mkdir -p build
	rm -f build/macos-SHA256SUMS
	find build/PPVPNCore.xcframework -type f -print0 | sort -z | xargs -0 shasum -a 256 > build/macos-SHA256SUMS

build-windows-artifact:
	mkdir -p build
	CGO_ENABLED=0 GOOS=windows GOARCH=amd64 go build -trimpath -o build/ppvpn-core-windows-amd64.exe ./cmd/ppvpn-core
	CGO_ENABLED=0 GOOS=windows GOARCH=arm64 go build -trimpath -o build/ppvpn-core-windows-arm64.exe ./cmd/ppvpn-core
	go version -m build/ppvpn-core-windows-amd64.exe | grep -Eq 'build[[:space:]]+GOOS=windows'
	go version -m build/ppvpn-core-windows-amd64.exe | grep -Eq 'build[[:space:]]+GOARCH=amd64'
	go version -m build/ppvpn-core-windows-arm64.exe | grep -Eq 'build[[:space:]]+GOOS=windows'
	go version -m build/ppvpn-core-windows-arm64.exe | grep -Eq 'build[[:space:]]+GOARCH=arm64'
	shasum -a 256 build/ppvpn-core-windows-amd64.exe build/ppvpn-core-windows-arm64.exe > build/windows-SHA256SUMS

build-desktop-artifact: build-macos-artifact build-windows-artifact

bootstrap-mobile:
	go install golang.org/x/mobile/cmd/gomobile@$(MOBILE_VERSION)
	go install golang.org/x/mobile/cmd/gobind@$(MOBILE_VERSION)

build-mobile-ios:
	mkdir -p build
	CLANG_MODULE_CACHE_PATH=$(CLANG_MODULE_CACHE_PATH) $(GO_BIN)/gomobile bind -target=ios -o build/PPVPNCore.xcframework ./mobile

verify-mobile-ios:
	scripts/verify-ios-xcframework.sh build/PPVPNCore.xcframework

build-ios-artifact: build-mobile-ios verify-mobile-ios
	rm -f build/PPVPNCore.xcframework.zip build/ios-SHA256SUMS
	ditto -c -k --sequesterRsrc --keepParent build/PPVPNCore.xcframework build/PPVPNCore.xcframework.zip
	shasum -a 256 build/PPVPNCore.xcframework.zip > build/ios-SHA256SUMS

build-mobile-macos:
	mkdir -p build
	scripts/build-macos-xcframework.sh

verify-mobile-macos:
	scripts/verify-macos-xcframework.sh build/PPVPNCore.xcframework

build-mobile-android:
	mkdir -p build
	$(GO_BIN)/gomobile bind -target=android -androidapi 23 -o build/ppvpn-core.aar ./mobile

verify-mobile-android:
	scripts/verify-android-aar.sh build/ppvpn-core.aar

build-android-artifact: build-mobile-android verify-mobile-android
	rm -f build/android-SHA256SUMS
	shasum -a 256 build/ppvpn-core.aar > build/android-SHA256SUMS
