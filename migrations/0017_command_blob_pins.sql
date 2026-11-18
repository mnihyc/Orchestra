-- Content that the machine of a transfer uploaded, or was told the system holds, for a command of a job: held for
-- the job until it ends, so that another transfer's release of the same content does not remove what this one needs.

CREATE TABLE command_blob_pins (
    command_id uuid NOT NULL,
    blob bytea NOT NULL,
    job_id uuid NOT NULL,
    pinned_at timestamp with time zone NOT NULL,
    CONSTRAINT command_blob_pins_pkey PRIMARY KEY (command_id, blob),
    CONSTRAINT command_blob_pins_blob_fkey FOREIGN KEY (blob) REFERENCES blobs(hash) ON DELETE CASCADE
);

CREATE INDEX command_blob_pins_job_idx ON command_blob_pins USING btree (job_id);
CREATE INDEX command_blob_pins_at_idx ON command_blob_pins USING btree (pinned_at);

COMMENT ON TABLE command_blob_pins IS 'One pin of blobs.pin_count for each blob a job''s command held for the job; released with the job''s end.';
