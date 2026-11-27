-- The host installed with the server enrolls in the name of the first administrator, for them alone: its Use row is cut
-- loose from the defaults. Owning a host of the system counts in no organization, so in 0.1.3 that row named nobody and
-- the first administrator could use the host only with As admin. Enrollment now names its maker in the row; this names
-- them in the row of a host that enrolled before, as enrollment would have, from when it enrolled.
-- Only a row nobody has written to since is completed: the host is still its maker's (the Owners row names them as it
-- was made), and its Use row never named anybody, now or before. Everything else stays as it is.
INSERT INTO permission_entries (id, object_kind, object_id, page, profile, permission, holder_kind, holder_id, options,
                                source, created_by, created_at)
SELECT gen_random_uuid(), 'host', h.id, 'live', 'any', 'host.use', 'user', h.owner_id, '{}'::jsonb, 'creation',
       h.owner_id, h.created_at
  FROM hosts h
  JOIN machines m ON m.id = h.machine_id AND m.deleted_at IS NULL
  JOIN permission_rows r ON r.object_kind = 'host' AND r.object_id = h.id AND r.page = 'live' AND r.profile = 'any'
                        AND r.permission = 'host.use' AND NOT r.inherit
 WHERE h.bundled_with_server
   AND h.home_id IS NULL
   AND h.owner_id IS NOT NULL
   AND EXISTS (SELECT 1 FROM permission_entries o
                WHERE o.object_kind = 'host' AND o.object_id = h.id AND o.page = 'live' AND o.permission = 'access.own'
                  AND o.holder_kind = 'user' AND o.holder_id = h.owner_id AND o.source = 'creation'
                  AND o.removed_at IS NULL)
   AND NOT EXISTS (SELECT 1 FROM permission_entries e
                    WHERE e.object_kind = 'host' AND e.object_id = h.id AND e.page = 'live'
                      AND e.permission = 'host.use');
