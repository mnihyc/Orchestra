WITH ancestors AS MATERIALIZED (
    SELECT DISTINCT unnest(path) AS id FROM nodes WHERE id = ANY($1)
), governance AS MATERIALIZED (
    -- Both ordinary Ask threads and private helpers can be looked up by thread ID.
    SELECT g.* FROM ask_governance g WHERE g.thread_id = ANY(ARRAY(SELECT id FROM ancestors))
), found AS MATERIALIZED (
    SELECT n.id, n.kind, n.parent_id, n.org_id, n.path, n.name, n.owner_id, n.archived_at, n.deleted_at,
           n.revision, e.placement, t.environment_id, n.readme_document_id IS NOT NULL AS has_readme,
           g.root_org_id AS ask_root_org_id,
           g.retention_days AS ask_retention_days, g.last_message_at AS ask_last_message_at,
           g.person_id AS ask_person_id, COALESCE(g.root_live,false) AS ask_root_live,
           COALESCE(g.inspectable,false) AS ask_inspectable
      FROM ancestors a JOIN nodes n ON n.id=a.id
      LEFT JOIN threads t ON t.node_id=n.id
      LEFT JOIN environments e ON e.id=t.environment_id
      LEFT JOIN governance g ON g.thread_id=t.node_id
), watched AS MATERIALIZED (
    SELECT orch_authz_saw(orch_authz_epochs(ARRAY(
        SELECT org_id FROM found UNION SELECT ask_root_org_id FROM found WHERE ask_root_org_id IS NOT NULL)))
     WHERE COALESCE(current_setting('orch.authz_seen', true), '') <> ''
)
SELECT found.* FROM found, (SELECT count(*) FROM watched) w
