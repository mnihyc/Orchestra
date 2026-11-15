-- Retain each real pool and lending audience independently of physical Host Use.
-- Entries may belong to permission_entries or pool_lending; removing an allowance
-- does not erase the provenance of a registration already made through it.
CREATE TABLE environment_creation_entries (
    environment_id uuid NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    entry_id uuid NOT NULL,
    PRIMARY KEY (environment_id, entry_id)
);
CREATE INDEX environment_creation_entries_audience_idx
    ON environment_creation_entries(entry_id, environment_id);
CREATE INDEX environments_live_host_entry_idx
    ON environments(host_entry_id, id) WHERE deleted_at IS NULL AND host_entry_id IS NOT NULL;
