//go:build windows

package localproxy

import (
	"os"

	"golang.org/x/sys/windows"
)

// The service-owned state is readable only by LocalSystem and local
// administrators. The protected DACL deliberately does not inherit broad
// ProgramData permissions.
const privateStateSDDL = "D:P(A;;FA;;;SY)(A;;FA;;;BA)"

func securePermissions(path string, _ os.FileInfo) bool {
	actual, err := windows.GetNamedSecurityInfo(
		path,
		windows.SE_FILE_OBJECT,
		windows.DACL_SECURITY_INFORMATION|windows.PROTECTED_DACL_SECURITY_INFORMATION,
	)
	if err != nil {
		return false
	}
	expected, err := windows.SecurityDescriptorFromString(privateStateSDDL)
	return err == nil && actual.String() == expected.String()
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
