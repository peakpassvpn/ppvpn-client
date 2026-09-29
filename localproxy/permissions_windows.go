//go:build windows

package localproxy

import (
	"os"
	"unsafe"

	"golang.org/x/sys/windows"
)

// The service-owned state is readable only by LocalSystem and local
// administrators. The protected DACL deliberately does not inherit broad
// ProgramData permissions.
const privateStateSDDL = "D:P(A;;FA;;;SY)(A;;FA;;;BA)"

// securePermissions reports whether path has a protected DACL that grants
// access only to LocalSystem and BUILTIN\Administrators.
//
// It checks the DACL semantically instead of comparing SDDL text. The
// SetNamedSecurityInfo inheritance model sets SE_DACL_AUTO_INHERITED, so a
// file written by applyPrivateACL can read back as "D:PAI(...)" rather than
// the "D:P(...)" it was set from; inheritable ACE flags on directories differ
// the same way. An exact string comparison then rejects state the core wrote
// itself, and apply-profile fails with a folded CORE_OPERATION_FAILED.
func securePermissions(path string, _ os.FileInfo) bool {
	descriptor, err := windows.GetNamedSecurityInfo(
		path,
		windows.SE_FILE_OBJECT,
		windows.DACL_SECURITY_INFORMATION|windows.PROTECTED_DACL_SECURITY_INFORMATION,
	)
	if err != nil {
		return false
	}
	return privateDescriptor(descriptor)
}

func privateDescriptor(descriptor *windows.SECURITY_DESCRIPTOR) bool {
	control, _, err := descriptor.Control()
	if err != nil || control&windows.SE_DACL_PROTECTED == 0 {
		return false
	}
	dacl, _, err := descriptor.DACL()
	if err != nil || dacl == nil {
		// A NULL DACL grants everyone full access.
		return false
	}
	allowed := 0
	for i := uint16(0); i < dacl.AceCount; i++ {
		var ace *windows.ACCESS_ALLOWED_ACE
		if err = windows.GetAce(dacl, uint32(i), &ace); err != nil {
			return false
		}
		switch ace.Header.AceType {
		case windows.ACCESS_DENIED_ACE_TYPE:
			continue // denying access never widens it
		case windows.ACCESS_ALLOWED_ACE_TYPE:
			sid := (*windows.SID)(unsafe.Pointer(&ace.SidStart))
			if !sid.IsWellKnown(windows.WinLocalSystemSid) && !sid.IsWellKnown(windows.WinBuiltinAdministratorsSid) {
				return false
			}
			allowed++
		default:
			// Object or callback ACEs are never written by this package.
			return false
		}
	}
	return allowed > 0
}

func secureDirectory(path string) error { return applyPrivateACL(path) }
func secureFile(path string) error      { return applyPrivateACL(path) }

func applyPrivateACL(path string) error {
	descriptor, err := windows.SecurityDescriptorFromString(privateStateSDDL)
	if err != nil {
		return err
	}
	dacl, _, err := descriptor.DACL()
	if err != nil {
		return err
	}
	return windows.SetNamedSecurityInfo(
		path,
		windows.SE_FILE_OBJECT,
		windows.DACL_SECURITY_INFORMATION|windows.PROTECTED_DACL_SECURITY_INFORMATION,
		nil,
		nil,
		dacl,
		nil,
	)
}
