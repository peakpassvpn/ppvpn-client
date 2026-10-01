package dnstransport

import "syscall"

// platformStaleErrors: on Windows a connection reset or abort surfaces as a
// Winsock errno, which syscall.ECONNRESET and syscall.ECONNABORTED (Go's
// invented values on Windows) do not match.
var platformStaleErrors = []error{syscall.WSAECONNRESET, syscall.WSAECONNABORTED}
