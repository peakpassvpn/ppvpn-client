// Package corelog is the core's first-party diagnostic log. sing-box's own
// logging stays disabled (its messages are not covered by the credential
// policy); this log carries lifecycle events and the causes behind folded
// API errors. Every line is written with one unbuffered write and, for files,
// flushed to disk, so a crash or a killed service child still leaves it.
package corelog

import (
	"errors"
	"fmt"
	"io"
	"os"
	"reflect"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/redact"
)

type Logger struct {
	mu sync.Mutex
	w  io.Writer
}

// New logs to w; a nil w discards.
func New(w io.Writer) *Logger {
	if w == nil {
		w = io.Discard
	}
	return &Logger{w: w}
}

// Discard is a logger that writes nothing.
func Discard() *Logger { return New(nil) }

// OpenFile appends to path (created 0600).
func OpenFile(path string) (*Logger, *os.File, error) {
	file, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return nil, nil, err
	}
	return New(file), file, nil
}

func (l *Logger) Info(msg string, fields ...any)  { l.write("info", msg, fields) }
func (l *Logger) Error(msg string, fields ...any) { l.write("error", msg, fields) }

func (l *Logger) write(level, msg string, fields []any) {
	if l == nil {
		return
	}
	var b strings.Builder
	b.WriteString(time.Now().UTC().Format(time.RFC3339Nano))
	b.WriteString(" level=")
	b.WriteString(level)
	b.WriteString(" msg=")
	b.WriteString(quote(msg))
	for i := 0; i+1 < len(fields); i += 2 {
		b.WriteByte(' ')
		b.WriteString(fmt.Sprint(fields[i]))
		b.WriteByte('=')
		b.WriteString(quote(fmt.Sprint(fields[i+1])))
	}
	b.WriteByte('\n')
	line := redact.Text(b.String())
	l.mu.Lock()
	defer l.mu.Unlock()
	_, _ = io.WriteString(l.w, line)
	if file, ok := l.w.(*os.File); ok {
		_ = file.Sync() // FlushFileBuffers on Windows; best effort for pipes
	}
}

func quote(s string) string {
	if s != "" && !strings.ContainsAny(s, " \t\n\"=") {
		return s
	}
	return fmt.Sprintf("%q", s)
}

// Chain describes err's wrap chain outermost first, with the dynamic type
// of each link and the numeric value of any OS error code, for example
// "*runtime.StageError > *fmt.wrapError > syscall.Errno(5)".
func Chain(err error) string {
	var links []string
	for depth := 0; err != nil && depth < 32; depth++ {
		link := reflect.TypeOf(err).String()
		var errno syscall.Errno
		if errors.As(err, &errno) && reflect.TypeOf(err) == reflect.TypeOf(errno) {
			link = fmt.Sprintf("%s(%d)", link, uintptr(errno))
		}
		links = append(links, link)
		if joined, ok := err.(interface{ Unwrap() []error }); ok {
			for _, inner := range joined.Unwrap() {
				links = append(links, "["+Chain(inner)+"]")
			}
			break
		}
		err = errors.Unwrap(err)
	}
	return strings.Join(links, " > ")
}
