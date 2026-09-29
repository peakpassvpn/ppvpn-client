package corelog

import (
	"bytes"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
)

type stage struct{ err error }

func (s *stage) Error() string { return "start: " + s.err.Error() }
func (s *stage) Unwrap() error { return s.err }

func TestLineFormatRedactionAndChain(t *testing.T) {
	var out bytes.Buffer
	log := New(&out)
	cause := &stage{fmt.Errorf("open tun: %w", syscall.Errno(5))}
	log.Error("operation failed", "stage", "start", "error", cause, "proxy", "http://user:secret@127.0.0.1:7890", "chain", Chain(cause))
	line := out.String()
	for _, want := range []string{" level=error ", `msg="operation failed"`, "stage=start", `error="start: open tun:`, "*corelog.stage > *fmt.wrapError > syscall.Errno(5)"} {
		if !strings.Contains(line, want) {
			t.Errorf("missing %q in %s", want, line)
		}
	}
	if strings.Contains(line, "secret") || !strings.HasSuffix(line, "\n") || strings.Count(line, "\n") != 1 {
		t.Fatalf("line: %q", line)
	}
	if Chain(nil) != "" || !strings.Contains(Chain(errors.Join(errors.New("a"), syscall.Errno(2))), "syscall.Errno(2)") {
		t.Fatal("chain edge cases")
	}
}

func TestFileLogIsWrittenImmediately(t *testing.T) {
	path := filepath.Join(t.TempDir(), "core.log")
	log, file, err := OpenFile(path)
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	log.Info("serve started", "version", "x")
	data, err := os.ReadFile(path)
	if err != nil || !strings.Contains(string(data), `msg="serve started" version=x`) {
		t.Fatalf("%q %v", data, err)
	}
	var nilLogger *Logger
	nilLogger.Error("ignored")
	Discard().Error("ignored")
}
