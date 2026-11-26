-- Shortcuts: a pointer to a thing by its kind and identity, kept in a person's own list or in a place.
-- A person's own list is what they starred, which this table held as `favorites`: every star stays, as a shortcut
-- with no place. A shortcut kept in a place (an org, a project or a topic) is seen by whoever may view the place and
-- opens its target only for whoever may open the target: it grants nothing. Moving or renaming the target changes
-- nothing here; deleting a shortcut never touches its target.
ALTER TABLE favorites RENAME TO shortcuts;
ALTER TABLE shortcuts RENAME COLUMN id TO target_id;
ALTER TABLE shortcuts RENAME CONSTRAINT favorites_kind_check TO shortcuts_kind_check;
ALTER TABLE shortcuts RENAME CONSTRAINT favorites_user_id_fkey TO shortcuts_user_id_fkey;
ALTER TABLE shortcuts DROP CONSTRAINT favorites_pkey;
DROP INDEX favorites_position_idx;

-- Its own identity, by which it is moved or removed; `user_id` is who made it, and in a person's own list whose it is.
ALTER TABLE shortcuts ADD COLUMN id uuid DEFAULT gen_random_uuid() NOT NULL;
ALTER TABLE shortcuts ADD CONSTRAINT shortcuts_pkey PRIMARY KEY (id);
-- The place it is kept in; none for a person's own list. It goes with the place when the place is purged.
ALTER TABLE shortcuts ADD COLUMN node_id uuid;
ALTER TABLE shortcuts ADD CONSTRAINT shortcuts_node_id_fkey FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE;
-- A place keeps shortcuts to documents, chats, threads, projects and topics; the view a project was starred with is a
-- person's own.
ALTER TABLE shortcuts ADD CONSTRAINT shortcuts_place_check CHECK (
    node_id IS NULL OR (kind = ANY (ARRAY['project'::text, 'topic'::text, 'thread'::text, 'document'::text, 'chat'::text]) AND view IS NULL)
);

-- One star per thing in a person's own list, and one shortcut per thing in a place, each list in its own order.
CREATE UNIQUE INDEX shortcuts_own_idx ON shortcuts USING btree (user_id, kind, target_id) WHERE (node_id IS NULL);
CREATE INDEX shortcuts_own_position_idx ON shortcuts USING btree (user_id, "position", kind, target_id) WHERE (node_id IS NULL);
CREATE UNIQUE INDEX shortcuts_place_idx ON shortcuts USING btree (node_id, kind, target_id) WHERE (node_id IS NOT NULL);
CREATE INDEX shortcuts_place_position_idx ON shortcuts USING btree (node_id, "position", id) WHERE (node_id IS NOT NULL);
