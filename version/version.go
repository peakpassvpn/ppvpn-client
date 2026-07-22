package version

const (
	CoreAPIVersion       = 1
	ProfileSchemaVersion = 1
	SingBoxVersion       = "1.13.12"
	CoreVersion          = "0.2.0"
)

type Info struct {
	CoreVersion          string `json:"core_version"`
	CoreAPIVersion       int    `json:"core_api_version"`
	ProfileSchemaVersion int    `json:"profile_schema_version"`
	SingBoxVersion       string `json:"sing_box_version"`
}

func Get() Info { return Info{CoreVersion, CoreAPIVersion, ProfileSchemaVersion, SingBoxVersion} }
