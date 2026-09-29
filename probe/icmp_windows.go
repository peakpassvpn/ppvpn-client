//go:build windows

package probe

import (
	"context"
	"crypto/rand"
	"encoding/binary"
	"errors"
	"net/netip"
	"syscall"
	"time"
	"unsafe"

	"golang.org/x/sys/windows"
)

// Windows exposes echo requests to unprivileged processes through the IP
// Helper API (IcmpSendEcho2 / Icmp6SendEcho2); no raw socket is used.
var (
	iphlpapi            = windows.NewLazySystemDLL("iphlpapi.dll")
	procIcmpCreateFile  = iphlpapi.NewProc("IcmpCreateFile")
	procIcmp6CreateFile = iphlpapi.NewProc("Icmp6CreateFile")
	procIcmpCloseHandle = iphlpapi.NewProc("IcmpCloseHandle")
	procIcmpSendEcho2   = iphlpapi.NewProc("IcmpSendEcho2")
	procIcmp6SendEcho2  = iphlpapi.NewProc("Icmp6SendEcho2")
)

// IP_STATUS values from ipexport.h (IPv6 aliases share these numbers).
const (
	ipSuccess             = 0
	ipStatusBase          = 11000
	ipDestNetUnreachable  = 11002 // also IP_DEST_NO_ROUTE
	ipDestHostUnreachable = 11003 // also IP_DEST_ADDR_UNREACHABLE
	ipDestProtUnreachable = 11004 // also IP_DEST_PROHIBITED
	ipDestPortUnreachable = 11005
	ipReqTimedOut         = 11010
	ipBadRoute            = 11012
	ipTTLExpiredTransit   = 11013 // also IP_HOP_LIMIT_EXCEEDED
	ipTTLExpiredReassem   = 11014
	ipBadDestination      = 11018
	ipDestUnreachable     = 11040
	ipTimeExceeded        = 11041
	ipDestScopeMismatch   = 11045
	ipStatusMax           = 11999
	echoPayloadSize       = 16
)

func Ping(ctx context.Context, addr netip.Addr, timeout time.Duration) (time.Duration, error) {
	addr = addr.Unmap()
	ms := timeout.Milliseconds()
	if ms < 1 {
		ms = 1
	}
	type outcome struct {
		rtt time.Duration
		err error
	}
	done := make(chan outcome, 1)
	// IcmpSendEcho2 blocks for at most the timeout; running it on its own
	// goroutine lets context cancellation return immediately. All buffers are
	// owned by that goroutine.
	go func() {
		rtt, err := sendEcho(addr, uint32(ms))
		done <- outcome{rtt, err}
	}()
	select {
	case result := <-done:
		return result.rtt, result.err
	case <-ctx.Done():
		if errors.Is(ctx.Err(), context.DeadlineExceeded) {
			return 0, &Error{Code: CodeICMPTimeout, Err: ctx.Err()}
		}
		return 0, ctx.Err()
	}
}

func sendEcho(addr netip.Addr, timeoutMS uint32) (time.Duration, error) {
	create := procIcmpCreateFile
	if addr.Is6() {
		create = procIcmp6CreateFile
	}
	if err := create.Find(); err != nil {
		return 0, &Error{Code: CodeICMPUnsupported, Err: err}
	}
	handle, _, callErr := create.Call()
	if windows.Handle(handle) == windows.InvalidHandle {
		return 0, &Error{Code: CodeICMPUnsupported, Err: callErr}
	}
	defer procIcmpCloseHandle.Call(handle)

	payload := make([]byte, echoPayloadSize)
	_, _ = rand.Read(payload)
	// ICMP_ECHO_REPLY(32/64) or ICMPV6_ECHO_REPLY + payload + an ICMP error
	// quote; 8 extra bytes are required by the API.
	reply := make([]byte, 128+len(payload)+8)
	started := time.Now()
	var count uintptr
	if addr.Is4() {
		ip := addr.As4()
		count, _, callErr = procIcmpSendEcho2.Call(
			handle, 0, 0, 0,
			uintptr(binary.LittleEndian.Uint32(ip[:])), // IPAddr is in network byte order in memory
			uintptr(unsafe.Pointer(&payload[0])), uintptr(len(payload)),
			0,
			uintptr(unsafe.Pointer(&reply[0])), uintptr(len(reply)),
			uintptr(timeoutMS),
		)
	} else {
		source := windows.RawSockaddrInet6{Family: windows.AF_INET6}
		destination := windows.RawSockaddrInet6{Family: windows.AF_INET6, Addr: addr.As16()}
		count, _, callErr = procIcmp6SendEcho2.Call(
			handle, 0, 0, 0,
			uintptr(unsafe.Pointer(&source)), uintptr(unsafe.Pointer(&destination)),
			uintptr(unsafe.Pointer(&payload[0])), uintptr(len(payload)),
			0,
			uintptr(unsafe.Pointer(&reply[0])), uintptr(len(reply)),
			uintptr(timeoutMS),
		)
	}
	rtt := time.Since(started)
	if count == 0 {
		var status uint32
		var errno syscall.Errno
		if errors.As(callErr, &errno) {
			status = uint32(errno)
		}
		return 0, statusError(status, callErr)
	}
	status := replyStatus(reply, addr.Is6())
	if status != ipSuccess {
		return 0, statusError(status, nil)
	}
	return rtt, nil
}

// replyStatus reads IP_STATUS from the first reply. ICMP_ECHO_REPLY (both the
// 32- and 64-bit layouts) starts with Address(4) Status(4). ICMPV6_ECHO_REPLY
// starts with a byte-packed 26-byte IPV6_ADDRESS_EX, so with natural
// alignment Status is at offset 28 after two zero padding bytes. Reading the
// four bytes at 26 is zero exactly when the status is IP_SUCCESS under either
// possible layout, which keeps success detection independent of packing.
func replyStatus(reply []byte, v6 bool) uint32 {
	if !v6 {
		return binary.LittleEndian.Uint32(reply[4:8])
	}
	packed := binary.LittleEndian.Uint32(reply[26:30])
	if packed == ipSuccess {
		return ipSuccess
	}
	if aligned := binary.LittleEndian.Uint32(reply[28:32]); aligned >= ipStatusBase && aligned <= ipStatusMax {
		return aligned
	}
	return packed
}

func statusError(status uint32, cause error) error {
	switch status {
	case ipReqTimedOut:
		return &Error{Code: CodeICMPTimeout, Err: cause}
	case ipDestNetUnreachable, ipDestHostUnreachable, ipDestProtUnreachable, ipDestPortUnreachable,
		ipBadRoute, ipTTLExpiredTransit, ipTTLExpiredReassem, ipBadDestination, ipDestUnreachable,
		ipTimeExceeded, ipDestScopeMismatch:
		return &Error{Code: CodeICMPUnreachable, Err: cause}
	case uint32(windows.ERROR_ACCESS_DENIED), uint32(windows.ERROR_NOT_SUPPORTED):
		return &Error{Code: CodeICMPUnsupported, Err: cause}
	default:
		if cause == nil {
			cause = syscall.Errno(status)
		}
		return &Error{Code: CodeICMPFailed, Err: cause}
	}
}
