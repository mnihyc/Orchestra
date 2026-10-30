-- Candidate upgrade: allocate a forward migration before applying to an installation.
-- The boolean key matches both live/deleted prepared predicates; id orders tied timestamps.
CREATE INDEX documents_node_page_idx
  ON documents (node_id, (deleted_at IS NOT NULL), updated_at DESC, id DESC);
