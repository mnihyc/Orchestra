WITH found AS MATERIALIZED (
             SELECT n.id, n.kind, n.parent_id, n.org_id, n.path, n.name, n.owner_id, n.archived_at, n.deleted_at,
                    n.revision, e.placement, t.environment_id, t.ask_root_org_id,
                    t.ask_retention_days, t.ask_last_message_at,
                    COALESCE(r.id IS NOT NULL AND r.deleted_at IS NULL AND r.kind='org',false) AS ask_root_live
               FROM nodes n
               LEFT JOIN threads t ON t.node_id = n.id
               LEFT JOIN environments e ON e.id = t.environment_id
               LEFT JOIN nodes r ON r.id=t.ask_root_org_id
              WHERE n.id IN (SELECT unnest(path) FROM nodes WHERE id = ANY($1))
         ), watched AS MATERIALIZED (
             SELECT orch_authz_saw(orch_authz_epochs(ARRAY(
                 SELECT org_id FROM found UNION SELECT ask_root_org_id FROM found WHERE ask_root_org_id IS NOT NULL)))
              WHERE COALESCE(current_setting('orch.authz_seen', true), '') <> ''
         )
         SELECT found.* FROM found, (SELECT count(*) FROM watched) w
