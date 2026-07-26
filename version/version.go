package version

const (
	CoreAPIVersion            = 1
	ProfileSchemaVersion      = 2
	FlowAdapterVersion        = 1
	LocalProxyContractVersion = 1
	CoreVersion               = "0.3.0"
)

type Info struct {
	CoreVersion               string `json:"core_version"`
	CoreAPIVersion            int    `json:"core_api_version"`
	ProfileSchemaVersion      int    `json:"profile_schema_version"`
	FlowAdapterVersion        int    `json:"flow_adapter_version"`
	LocalProxyContractVersion int    `json:"local_proxy_contract_version"`
}

func Get() Info {
	return Info{
		CoreVersion:               CoreVersion,
		CoreAPIVersion:            CoreAPIVersion,
		ProfileSchemaVersion:      ProfileSchemaVersion,
		FlowAdapterVersion:        FlowAdapterVersion,
		LocalProxyContractVersion: LocalProxyContractVersion,
	}
}
