package profile

// Migrate is deliberately explicit: callers must fetch a Profile v2 rather
// than silently losing v2 routing semantics while upgrading an older profile.
func Migrate(p *Profile) (*Profile, error) {
	if p == nil || p.SchemaVersion == CurrentSchemaVersion {
		return p, nil
	}
	return nil, invalid("SCHEMA_UNSUPPORTED", "schema_version", "no migration path exists")
}
