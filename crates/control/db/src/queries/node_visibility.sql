reader_owned AS MATERIALIZED (
  SELECT object_id FROM permission_entries
   WHERE object_kind = 'node' AND page = 'live' AND permission = 'access.own'
     AND removed_at IS NULL AND (expires_at IS NULL OR expires_at > __AT__)
     AND ((holder_kind = 'user' AND holder_id = __USER__)
       OR (holder_kind = 'group' AND holder_id = ANY(__GROUPS__::uuid[])))
), participating_nodes AS MATERIALIZED (
  SELECT jsonb_array_elements_text(COALESCE(__GRANTS__::jsonb->'participating_nodes','[]'::jsonb))::uuid id
), participating_chats AS MATERIALIZED (
  SELECT jsonb_array_elements_text(COALESCE(__GRANTS__::jsonb->'participating_chats','[]'::jsonb))::uuid id
), node_access AS NOT MATERIALIZED (
  SELECT n.*, t.state AS thread_state, t.last_activity_at AS thread_activity_at,
         CASE WHEN pn.id IS NULL AND pc.id IS NULL THEN page.value ELSE
           jsonb_set(jsonb_set(page.value,'{participating}',
             to_jsonb(pn.id IS NOT NULL OR COALESCE((page.value->>'participating')::boolean,false))),
             '{chat,participating}',to_jsonb(pc.id IS NOT NULL OR COALESCE((page.value->'chat'->>'participating')::boolean,false))) END AS "grant"
    -- node_id is unique. Keep this lookup parameterized when a parent probes its first visible child;
    -- flattening it lets the planner scan the whole thread table once per parent.
    FROM nodes n LEFT JOIN LATERAL (
      SELECT t.state,t.last_activity_at,t.environment_id,t.side_of FROM threads t
       WHERE n.kind='thread' AND t.node_id=n.id LIMIT 1
    ) t ON true
    LEFT JOIN environments e ON e.id = t.environment_id
    LEFT JOIN participating_nodes pn ON pn.id=n.id
    LEFT JOIN participating_chats pc ON pc.id=n.id
    -- Extract the final small grant directly, without copying the full projection's intermediate maps.
    CROSS JOIN LATERAL (SELECT COALESCE(__GRANTS__::jsonb #> ARRAY['objects',n.id::text],
           __GRANTS__::jsonb #> ARRAY['threads',n.parent_id::text,
             COALESCE(__GRANTS__::jsonb #>> ARRAY['environments',e.id::text], e.placement, 'managed'),
             CASE WHEN n.archived_at IS NOT NULL THEN 'archived' ELSE 'live' END,
             CASE WHEN n.org_id = ANY(__ROOTS__::uuid[]) AND n.id IN (SELECT object_id FROM reader_owned)
               THEN 'owned' ELSE 'ordinary' END],
           '{"view":false,"named":false,"access":"path_only","share":false,"settings":false}'::jsonb) value) page
   WHERE n.deleted_at IS NULL AND t.side_of IS NULL
     AND NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id = ANY(n.path) AND gone.deleted_at IS NOT NULL)
)
