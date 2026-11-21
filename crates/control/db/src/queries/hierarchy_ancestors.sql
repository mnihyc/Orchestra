WITH ancestors AS MATERIALIZED (
    SELECT n.id, n.kind, n.parent_id, n.org_id, n.path, n.name, n.owner_id, n.archived_at,
           n.deleted_at, n.revision, t.environment_id, t.node_id AS thread_id, t.ask_root_org_id
      FROM (SELECT DISTINCT unnest(path) AS id FROM nodes WHERE id = ANY($1)) a
      JOIN nodes n ON n.id=a.id LEFT JOIN threads t ON t.node_id=n.id
), governance AS MATERIALIZED (
    SELECT g.* FROM ancestor_ask_governance((
        SELECT array_agg(candidate.id) FROM (
            SELECT a.thread_id AS id FROM ancestors a WHERE a.ask_root_org_id IS NOT NULL
            UNION ALL
            SELECT w.helper_id FROM delegated_program_work w WHERE w.purpose='ask'
               AND w.helper_id=ANY(ARRAY(SELECT thread_id FROM ancestors WHERE thread_id IS NOT NULL))
        ) candidate
    )) g
)
SELECT a.id, a.kind, a.parent_id, a.org_id, a.path, a.name, a.owner_id, a.archived_at, a.deleted_at,
       a.revision, e.placement, a.environment_id, g.root_org_id AS ask_root_org_id,
       g.retention_days AS ask_retention_days, g.last_message_at AS ask_last_message_at,
       g.person_id AS ask_person_id, COALESCE(g.root_live,false) AS ask_root_live,
       COALESCE(g.inspectable,false) AS ask_inspectable
  FROM ancestors a LEFT JOIN environments e ON e.id=a.environment_id
  LEFT JOIN governance g ON g.thread_id=a.thread_id
