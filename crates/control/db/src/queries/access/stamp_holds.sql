SELECT $4::text IS NOT NULL
                AND COALESCE((SELECT oid::int8 FROM pg_database WHERE datname = current_database()) = $5, false)
                AND pg_snapshot_xmin($4::pg_snapshot) > COALESCE((SELECT floor FROM access_changes_floor WHERE id), '0'::xid8)
                AND pg_snapshot_xmax($4::pg_snapshot) <= pg_snapshot_xmax(pg_current_snapshot())
                AND NOT EXISTS (SELECT 1 FROM pg_snapshot_xip(pg_current_snapshot()) AS running(xid)
                                 WHERE pg_visible_in_snapshot(running.xid, $4::pg_snapshot))
                AND NOT EXISTS (SELECT 1 FROM access_changes c WHERE c.xid >= pg_snapshot_xmax($4::pg_snapshot))
                AND NOT EXISTS (SELECT 1 FROM access_changes c
                                 WHERE c.xid = ANY (ARRAY(SELECT pg_snapshot_xip($4::pg_snapshot))))
