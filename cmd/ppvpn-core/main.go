package main

import (
	"context"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"path/filepath"
	goruntime "runtime"
	"syscall"
	"time"

	"github.com/peakpassvpn/ppvpn-core/api"
	"github.com/peakpassvpn/ppvpn-core/internal/config"
	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/peakpassvpn/ppvpn-core/internal/failover"
	"github.com/peakpassvpn/ppvpn-core/internal/redact"
	coreruntime "github.com/peakpassvpn/ppvpn-core/internal/runtime"
	"github.com/peakpassvpn/ppvpn-core/ipc"
	"github.com/peakpassvpn/ppvpn-core/probe"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/peakpassvpn/ppvpn-core/version"
	singjson "github.com/sagernet/sing/common/json"
)

func main() {
	if err := run(os.Args[1:]); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
func run(args []string) error {
	if len(args) == 0 {
		return fmt.Errorf("usage: ppvpn-core <version|validate|render|probe-entrance|serve>")
	}
	switch args[0] {
	case "version":
		return json.NewEncoder(os.Stdout).Encode(version.Get())
	case "validate":
		return validate(args[1:])
	case "render":
		return render(args[1:])
	case "probe-entrance":
		return probeEntrance(args[1:])
	case "serve":
		return serve(args[1:])
	default:
		return fmt.Errorf("unknown command %q", args[0])
	}
}

func readProfile(path string) (*profile.Profile, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	p, err := profile.Parse(data)
	if err != nil {
		return nil, err
	}
	return p, nil
}
func render(args []string) error {
	flags := flag.NewFlagSet("render", flag.ContinueOnError)
	platformName := flags.String("platform", "macos", "platform name")
	if err := flags.Parse(args); err != nil {
		return err
	}
	if flags.NArg() != 1 {
		return fmt.Errorf("usage: ppvpn-core render [--platform macos] profile.json")
	}
	p, err := readProfile(flags.Arg(0))
	if err != nil {
		return err
	}
	built, err := config.Build(p, profile.PlatformCapabilities{Platform: *platformName, LogLevel: "info"}, time.Now())
	if err != nil {
		return err
	}
	data, err := singjson.MarshalContext(failover.Context(context.Background()), built.Options)
	if err != nil {
		return fmt.Errorf("render configuration failed")
	}
	var output any
	if err = json.Unmarshal(redact.JSON(data), &output); err != nil {
		return err
	}
	encoder := json.NewEncoder(os.Stdout)
	encoder.SetIndent("", "  ")
	return encoder.Encode(output)
}
func probeEntrance(args []string) error {
	flags := flag.NewFlagSet("probe-entrance", flag.ContinueOnError)
	timeout := flags.Duration("timeout", 5*time.Second, "per-ingress timeout")
	concurrency := flags.Int("concurrency", 4, "maximum concurrent ingress probes")
	methodName := flags.String("method", "tcp", "probe method: tcp (TCP connect) or icmp (unprivileged echo)")
	if err := flags.Parse(args); err != nil {
		return err
	}
	if flags.NArg() != 1 {
		return fmt.Errorf("usage: ppvpn-core probe-entrance [--method tcp|icmp] [--timeout 5s] [--concurrency 4] profile.json")
	}
	method, err := probe.ParseMethod(*methodName)
	if err != nil {
		return err
	}
	p, err := readProfile(flags.Arg(0))
	if err != nil {
		return err
	}
	results, err := probe.Entrances(context.Background(), p, probe.Options{Method: method, Timeout: *timeout, Concurrency: *concurrency})
	if err != nil {
		return err
	}
	return json.NewEncoder(os.Stdout).Encode(results)
}

func validate(args []string) error {
	if len(args) != 1 {
		return fmt.Errorf("usage: ppvpn-core validate profile.json")
	}
	p, err := readProfile(args[0])
	if err != nil {
		return err
	}
	if err = profile.Validate(p, time.Now()); err != nil {
		return err
	}
	fmt.Println("profile valid")
	return nil
}

func serve(args []string) error {
	flags := flag.NewFlagSet("serve", flag.ContinueOnError)
	socket := flags.String("socket", "", "Unix socket or Windows named pipe path")
	secretFile := flags.String("session-secret-file", "", "private file used to exchange the random session secret")
	stateDir := flags.String("state-dir", "", "private directory for device-local state")
	platformName := flags.String("platform", "desktop", "platform capability name")
	localProxy := flags.Bool("local-proxy", true, "enable the shared authenticated local HTTP/SOCKS5 proxy (one port, node chosen by username)")
	tun := flags.Bool("tun", false, "enable sing-box TUN inbound (requires host-provided privileges)")
	tunStack := flags.String("tun-stack", "mixed", "sing-box TUN stack: mixed, system, or gvisor")
	exitOnStdin := flags.Bool("exit-on-stdin-close", false, "exit when the parent-owned stdin pipe closes")
	logFile := flags.String("log-file", "", "append the core diagnostic log to this file (default: stderr)")
	if err := flags.Parse(args); err != nil {
		return err
	}
	log := corelog.New(os.Stderr)
	if *logFile != "" {
		fileLog, file, err := corelog.OpenFile(*logFile)
		if err != nil {
			return fmt.Errorf("open log file: %w", err)
		}
		defer file.Close()
		log = fileLog
	}
	err := serveWithLog(log, *socket, *secretFile, *stateDir, *platformName, *localProxy, *tun, *tunStack, *exitOnStdin)
	if err != nil {
		log.Error("serve failed", "error", err, "chain", corelog.Chain(err))
	}
	return err
}

func serveWithLog(log *corelog.Logger, socket, secretFile, stateDir, platformName string, localProxy, tun bool, tunStack string, exitOnStdinClose bool) error {
	info := version.Get()
	log.Info("serve starting", "core_version", info.CoreVersion, "os", goruntime.GOOS, "arch", goruntime.GOARCH,
		"platform", platformName, "tun", tun, "tun_stack", tunStack, "local_proxy", localProxy, "state_dir", stateDir, "socket", socket)
	if socket == "" || secretFile == "" || stateDir == "" {
		return fmt.Errorf("serve requires --socket, --session-secret-file and --state-dir")
	}
	if err := os.MkdirAll(stateDir, 0o700); err != nil {
		return fmt.Errorf("create state dir: %w", err)
	}
	secret, err := rotateSessionSecret(secretFile)
	if err != nil {
		return fmt.Errorf("write session secret: %w", err)
	}
	defer os.Remove(secretFile)
	capabilities := profile.PlatformCapabilities{
		Platform:   platformName,
		TUN:        profile.TUNCapabilities{Enabled: tun, Stack: tunStack},
		LocalProxy: profile.LocalProxyCapabilities{Enabled: localProxy, Listen: "127.0.0.1"},
		LogLevel:   "info",
	}
	core := coreruntime.NewWithLocalProxyState(capabilities, filepath.Join(stateDir, "local-proxies.json"))
	server, err := api.NewServer(core, secret)
	if err != nil {
		return err
	}
	server.SetLogger(log)
	desktop, err := ipc.Listen(socket, server.Handler())
	if err != nil {
		return fmt.Errorf("listen on %s: %w", socket, err)
	}
	log.Info("serve ready", "socket", socket)
	serveDone := make(chan error, 1)
	go func() { serveDone <- desktop.Serve() }()
	signals := make(chan os.Signal, 1)
	signal.Notify(signals, os.Interrupt, syscall.SIGTERM)
	defer signal.Stop(signals)
	var stdinClosed <-chan struct{}
	if exitOnStdinClose {
		ch := make(chan struct{})
		stdinClosed = ch
		go func() { _, _ = io.Copy(io.Discard, os.Stdin); close(ch) }()
	}
	select {
	case err = <-serveDone:
		return fmt.Errorf("ipc serve: %w", err)
	case <-signals:
		log.Info("serve stopping", "reason", "signal")
	case <-stdinClosed:
		log.Info("serve stopping", "reason", "stdin closed")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_ = core.Stop()
	return desktop.Shutdown(ctx)
}

func rotateSessionSecret(path string) (string, error) {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return "", err
	}
	bytes := make([]byte, 32)
	if _, err := rand.Read(bytes); err != nil {
		return "", err
	}
	secret := base64.RawURLEncoding.EncodeToString(bytes)
	tmp, err := os.CreateTemp(filepath.Dir(path), ".session-*")
	if err != nil {
		return "", err
	}
	name := tmp.Name()
	defer os.Remove(name)
	if err = tmp.Chmod(0o600); err == nil {
		_, err = tmp.WriteString(secret)
	}
	if closeErr := tmp.Close(); err == nil {
		err = closeErr
	}
	if err != nil {
		return "", err
	}
	if err = os.Rename(name, path); err != nil {
		return "", err
	}
	return secret, nil
}
