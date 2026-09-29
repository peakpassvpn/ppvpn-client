//go:build windows

package privateacl

import (
	"errors"
	"fmt"
	"unsafe"

	"golang.org/x/sys/windows"
)

// fileAllAccess is FILE_ALL_ACCESS ("FA" in SDDL).
const fileAllAccess = windows.STANDARD_RIGHTS_REQUIRED | windows.SYNCHRONIZE | 0x1FF

// currentUser returns the SID of the process token's user. Tests replace it.
var currentUser = func() (*windows.SID, error) {
	user, err := windows.GetCurrentProcessToken().GetTokenUser()
	if err != nil {
		return nil, fmt.Errorf("read process token user: %w", err)
	}
	return user.User.Sid.Copy()
}

// trustees returns the SIDs a private object grants access to for a process
// running as user: LocalSystem and Administrators for LocalSystem itself,
// otherwise that user and LocalSystem.
func trustees(user *windows.SID) ([]*windows.SID, error) {
	system, err := windows.CreateWellKnownSid(windows.WinLocalSystemSid)
	if err != nil {
		return nil, err
	}
	if user.IsWellKnown(windows.WinLocalSystemSid) {
		admins, err := windows.CreateWellKnownSid(windows.WinBuiltinAdministratorsSid)
		if err != nil {
			return nil, err
		}
		return []*windows.SID{system, admins}, nil
	}
	return []*windows.SID{user, system}, nil
}

// legacyTrustees is the fixed ACL ppvpn-core 0.4.0 wrote for every account.
func legacyTrustees() ([]*windows.SID, error) {
	system, err := windows.CreateWellKnownSid(windows.WinLocalSystemSid)
	if err != nil {
		return nil, err
	}
	admins, err := windows.CreateWellKnownSid(windows.WinBuiltinAdministratorsSid)
	if err != nil {
		return nil, err
	}
	return []*windows.SID{system, admins}, nil
}

func currentTrustees() ([]*windows.SID, bool, error) {
	user, err := currentUser()
	if err != nil {
		return nil, false, err
	}
	sids, err := trustees(user)
	return sids, user.IsWellKnown(windows.WinLocalSystemSid), err
}

func privateACL(sids []*windows.SID, directory bool) (*windows.ACL, error) {
	inheritance := uint32(windows.NO_INHERITANCE)
	if directory {
		// Files created inside (temp files before they are re-ACLed) start
		// with the same private ACEs instead of nothing.
		inheritance = windows.SUB_CONTAINERS_AND_OBJECTS_INHERIT
	}
	entries := make([]windows.EXPLICIT_ACCESS, len(sids))
	for i, sid := range sids {
		entries[i] = windows.EXPLICIT_ACCESS{
			AccessPermissions: fileAllAccess,
			AccessMode:        windows.GRANT_ACCESS,
			Inheritance:       inheritance,
			Trustee: windows.TRUSTEE{
				TrusteeForm:  windows.TRUSTEE_IS_SID,
				TrusteeType:  windows.TRUSTEE_IS_UNKNOWN,
				TrusteeValue: windows.TrusteeValueFromSID(sid),
			},
		}
	}
	return windows.ACLFromEntries(entries, nil)
}

func apply(path string, directory bool) error {
	sids, _, err := currentTrustees()
	if err != nil {
		return err
	}
	acl, err := privateACL(sids, directory)
	if err != nil {
		return err
	}
	// Setting the DACL needs WRITE_DAC, which the owner always has, so this
	// also repairs objects whose DACL no longer grants this user anything.
	err = windows.SetNamedSecurityInfo(path, windows.SE_FILE_OBJECT,
		windows.DACL_SECURITY_INFORMATION|windows.PROTECTED_DACL_SECURITY_INFORMATION, nil, nil, acl, nil)
	if errors.Is(err, windows.ERROR_ACCESS_DENIED) {
		return accessError(path, err)
	}
	return err
}

func accessError(path string, err error) error {
	return &AccessError{Path: path, Err: err, Remedy: "this account is not its owner and cannot change its ACL " +
		"(ppvpn-core 0.4.0 created it with an administrators-only ACL); from an elevated prompt run " +
		`icacls "` + path + `" /reset /t /c, or delete it, then restart the app. Nothing was deleted`}
}

// SecureDirectory applies the private ACL to an existing directory.
func SecureDirectory(path string) error { return apply(path, true) }

// SecureFile applies the private ACL to an existing file.
func SecureFile(path string) error { return apply(path, false) }

// CheckFile reports whether path exists and, if so, that it is private to
// this account. A file carrying the 0.4.0 administrators-only ACL is
// repaired in place when this account owns it; one that grants anyone else
// access fails with ErrNotPrivate.
func CheckFile(path string) (bool, error) {
	descriptor, err := windows.GetNamedSecurityInfo(path, windows.SE_FILE_OBJECT,
		windows.DACL_SECURITY_INFORMATION|windows.PROTECTED_DACL_SECURITY_INFORMATION)
	switch {
	case errors.Is(err, windows.ERROR_FILE_NOT_FOUND), errors.Is(err, windows.ERROR_PATH_NOT_FOUND):
		return false, nil
	case errors.Is(err, windows.ERROR_ACCESS_DENIED):
		return true, accessError(path, err)
	case err != nil:
		return false, err
	}
	sids, system, err := currentTrustees()
	if err != nil {
		return true, err
	}
	if privateFor(descriptor, sids) {
		return true, nil
	}
	legacy, err := legacyTrustees()
	if err != nil {
		return true, err
	}
	if !system && privateFor(descriptor, legacy) {
		// Narrower than ours: only SYSTEM/Administrators. Granting the
		// owning user access does not expose the secret to anyone new.
		return true, SecureFile(path)
	}
	return true, fmt.Errorf("%s: %w", path, ErrNotPrivate)
}

// privateFor reports whether descriptor has a protected DACL whose allow
// ACEs all name one of allowed. Deny ACEs never widen access and are
// ignored; object and callback ACEs are rejected.
func privateFor(descriptor *windows.SECURITY_DESCRIPTOR, allowed []*windows.SID) bool {
	control, _, err := descriptor.Control()
	if err != nil || control&windows.SE_DACL_PROTECTED == 0 {
		return false
	}
	dacl, _, err := descriptor.DACL()
	if err != nil || dacl == nil {
		return false // a NULL DACL grants everyone full access
	}
	granted := 0
	for i := uint16(0); i < dacl.AceCount; i++ {
		var ace *windows.ACCESS_ALLOWED_ACE
		if err = windows.GetAce(dacl, uint32(i), &ace); err != nil {
			return false
		}
		switch ace.Header.AceType {
		case windows.ACCESS_DENIED_ACE_TYPE:
			continue
		case windows.ACCESS_ALLOWED_ACE_TYPE:
			sid := (*windows.SID)(unsafe.Pointer(&ace.SidStart))
			known := false
			for _, candidate := range allowed {
				known = known || sid.Equals(candidate)
			}
			if !known {
				return false
			}
			granted++
		default:
			return false
		}
	}
	return granted > 0
}
