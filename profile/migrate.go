package profile

// Migrate is deliberately explicit: schema 1 is current and unknown versions fail closed.
func Migrate(p *Profile) (*Profile, error) {
	if p == nil || p.SchemaVersion == CurrentSchemaVersion {
		return p, nil
	}
	return nil, invalid("SCHEMA_UNSUPPORTED", "schema_version", "no migration path exists")
}
