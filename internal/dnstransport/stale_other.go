//go:build !windows

package dnstransport

// platformStaleErrors: elsewhere the syscall errnos in staleErrors are the
// real ones.
var platformStaleErrors []error
