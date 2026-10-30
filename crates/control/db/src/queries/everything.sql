-- Keep the reader's large projection in one parameter source, rather than expanding it in every branch.
WITH reader_projection AS MATERIALIZED (SELECT $4::jsonb AS grants), __NODE_VISIBILITY__, stored_sizes AS MATERIALIZED (
  -- Only recorded readable document storage is aggregated; private bytes never contribute.
  SELECT ancestor.id, sum(d.bytes + d.files_bytes)::bigint size_bytes
    FROM documents d JOIN nodes n ON n.id = d.node_id
    CROSS JOIN LATERAL unnest(n.path) ancestor(id)
   WHERE d.deleted_at IS NULL AND COALESCE((__GRANTS__->'objects'->d.id::text->>'view')::boolean,false)
     AND NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id = ANY(n.path) AND gone.deleted_at IS NOT NULL)
   GROUP BY ancestor.id
), node_rows AS NOT MATERIALIZED (
  SELECT n.kind, n.id::text id, n.name, n.parent_id::text parent_id, n.path, n.org_id,
         n.created_by, n.owner_id, n.created_at,
         GREATEST(n.activity_at,n.updated_at,n.created_at,n.thread_activity_at) last_activity,
         n.thread_state state, (SELECT measured.size_bytes FROM stored_sizes measured WHERE measured.id = n.id) size_bytes, NULL::jsonb takes,
         n."grant",
         n.id uuid_key
    FROM node_access n
), resource_rows AS NOT MATERIALIZED (
  SELECT 'environment'::text kind, e.id, e.name, e.home_node_id home, e.org_id, e.owner_id,
         e.created_at, e.last_activity_at last_activity, e.status state, e.retained_bytes size_bytes,
         -- The route uses the existing needs/limits resolver over these non-secret recorded fields.
         CASE WHEN e.placement = 'managed' AND t.id IS NOT NULL AND COALESCE((__GRANTS__->'objects'->e.id::text->>'view')::boolean,false)
           THEN jsonb_build_object('spec',jsonb_build_object('needs',t.spec->'needs','resources',t.spec->'resources','resources_max',t.spec->'resources_max'),
             'config',(SELECT COALESCE(jsonb_object_agg(own.key,own.value),'{}'::jsonb) FROM jsonb_each(e.config) own WHERE own.key IN ('env.needs','env.resources')),
             'revision',e.revision) END takes
    FROM environments e LEFT JOIN env_templates t ON t.id = e.template_id WHERE e.deleted_at IS NULL
  UNION ALL SELECT 'provider', id, name, home_id, org_id, owner_id, created_at, updated_at, status, NULL, NULL FROM providers WHERE deleted_at IS NULL
  UNION ALL SELECT 'agent', id, name, home_id, org_id, owner_id, created_at, updated_at, CASE WHEN disabled THEN 'disabled' ELSE 'enabled' END, NULL, NULL FROM agents WHERE deleted_at IS NULL
  UNION ALL SELECT 'mcp_server', id, name, home_id, org_id, owner_id, created_at, updated_at, CASE WHEN enabled THEN 'enabled' ELSE 'disabled' END, NULL, NULL FROM mcp_servers WHERE deleted_at IS NULL AND environment_id IS NULL
  UNION ALL SELECT 'host', h.id, h.name, h.home_id, h.org_id, h.owner_id, h.created_at, h.load_at, CASE WHEN h.schedulable AND NOT h.disabled THEN 'open' ELSE 'closed' END, NULL, NULL FROM hosts h JOIN machines m ON m.id = h.machine_id WHERE m.deleted_at IS NULL
  UNION ALL SELECT 'template', id, name, home_id, org_id, owner_id, created_at, updated_at, CASE WHEN enabled THEN 'enabled' ELSE 'disabled' END, NULL, NULL FROM env_templates WHERE deleted_at IS NULL
  UNION ALL SELECT 'mount', id, name, home_id, org_id, owner_id, created_at, updated_at, CASE WHEN enabled THEN 'enabled' ELSE 'disabled' END, NULL, NULL FROM mounts WHERE deleted_at IS NULL
  UNION ALL SELECT 'skill', id, name, home_id, org_id, owner_id, created_at, updated_at, CASE WHEN enabled THEN 'enabled' ELSE 'disabled' END, NULL, NULL FROM skills WHERE deleted_at IS NULL
), inventory AS NOT MATERIALIZED (
  SELECT kind,id,name,parent_id,path,org_id,created_by,owner_id,created_at,last_activity,state,size_bytes,takes,"grant" FROM node_rows
  UNION ALL
  SELECT 'document', d.id::text, d.title, d.node_id::text, n.path, d.org_id,
         d.created_by, d.owner_id, d.created_at, d.updated_at, NULL, d.bytes + d.files_bytes, NULL,
         COALESCE(__GRANTS__->'objects'->d.id::text, '{}'::jsonb)
    FROM documents d JOIN nodes n ON n.id = d.node_id
   WHERE d.deleted_at IS NULL
     AND NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id = ANY(n.path) AND gone.deleted_at IS NOT NULL)
  UNION ALL
  -- Keep node paths plain and the shared parent expression intact, so both existing indexes remain usable.
  SELECT 'chat', c.id::text, COALESCE(c.name, n.name), COALESCE(c.node_id, c.org_id)::text, n.path, c.org_id,
         c.created_by, NULL, c.created_at, c.last_message_at, NULL, NULL, NULL,
         COALESCE(n."grant"->'chat', '{}'::jsonb)
    FROM chats c JOIN nodes root ON root.id = c.org_id JOIN node_rows n ON n.uuid_key = c.node_id
   WHERE c.node_id IS NOT NULL
     AND NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id = ANY(n.path) AND gone.deleted_at IS NOT NULL)
  UNION ALL
  SELECT 'chat', c.id::text,
         COALESCE(c.name, (SELECT string_agg(u.display_name, ', ' ORDER BY u.username) FROM chat_members cm JOIN users u ON u.id = cm.user_id WHERE cm.chat_id = c.id AND cm.left_at IS NULL), ''),
         COALESCE(c.node_id, c.org_id)::text, '{}'::uuid[], c.org_id,
         c.created_by, NULL, c.created_at, c.last_message_at, NULL, NULL, NULL,
         COALESCE(__GRANTS__->'objects'->c.id::text, '{}'::jsonb)
    FROM chats c JOIN nodes root ON root.id = c.org_id
   WHERE c.node_id IS NULL
     AND NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id = ANY(root.path) AND gone.deleted_at IS NOT NULL)
  UNION ALL
  SELECT r.kind, r.id::text, r.name, 'resources', COALESCE(n.path, '{}'::uuid[]), r.org_id,
         NULL, r.owner_id, r.created_at, r.last_activity, r.state, r.size_bytes, r.takes,
         COALESCE(__GRANTS__->'objects'->r.id::text, '{}'::jsonb)
    FROM resource_rows r LEFT JOIN nodes n ON n.id = r.home
   WHERE n.id IS NULL OR NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id = ANY(n.path) AND gone.deleted_at IS NOT NULL)
  UNION ALL
  SELECT 'machine', m.id::text, m.name, 'resources', COALESCE(n.path,'{}'::uuid[]), CASE WHEN h.id IS NULL THEN m.org_id ELSE h.org_id END,
         NULL, m.owner_id, m.created_at, m.last_seen_at, CASE WHEN m.status = 'revoked' THEN 'revoked' WHEN m.online THEN 'online' ELSE 'offline' END, NULL, NULL,
         COALESCE(__GRANTS__->'objects'->COALESCE(h.id,m.id)::text,'{}'::jsonb)
    FROM machines m LEFT JOIN hosts h ON h.machine_id=m.id
    LEFT JOIN nodes n ON n.id=CASE WHEN h.id IS NULL THEN m.home_id ELSE h.home_id END
   WHERE m.runtime_id IS NULL AND m.deleted_at IS NULL
      AND (n.id IS NULL OR NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id=ANY(n.path) AND gone.deleted_at IS NOT NULL))
), authorized AS NOT MATERIALIZED (
  SELECT i.*, ((COALESCE((i."grant"->>'view')::boolean,false)
      OR (COALESCE((i."grant"->>'listed')::boolean,false) AND $9::uuid IS NULL AND $10::timestamptz IS NULL AND $11::timestamptz IS NULL AND $29::timestamptz IS NULL
        AND $30::text IS NULL AND $31::text IS NULL AND $32::text IS NULL AND NOT $33::boolean AND $34::uuid IS NULL))
    AND (cardinality($8::text[]) = 0 OR i.kind = ANY($8))
    AND ($9::uuid IS NULL OR i.owner_id = $9)
    AND ($10::timestamptz IS NULL OR i.created_at >= $10)
    AND ($11::timestamptz IS NULL OR i.created_at <= $11)
    AND ($29::timestamptz IS NULL OR i.last_activity >= $29)
    AND ($30::text IS NULL OR i.state = $30)
    AND ($31::text IS NULL OR i.kind = 'agent' AND EXISTS (SELECT 1 FROM agents a WHERE a.id=i.id::uuid AND a.harness_kind=$31))
    AND ($32::text IS NULL OR CASE i.kind
      WHEN 'template' THEN EXISTS (SELECT 1 FROM env_templates t WHERE t.id=i.id::uuid AND t.kind=$32)
      WHEN 'environment' THEN EXISTS (SELECT 1 FROM environments e WHERE e.id=i.id::uuid AND CASE WHEN e.template_id IS NULL THEN 'machine' ELSE 'template' END=$32)
      ELSE false END)
    AND (NOT $33::boolean OR COALESCE((i."grant"->>'library_scope')::boolean,false))
    AND ($34::uuid IS NULL OR i.path @> ARRAY[$34])) fits
    FROM inventory i
), filter_paths AS MATERIALIZED (
  -- A match itself uses its direct filter flag; only ancestors need a separate reachability set.
  SELECT DISTINCT ancestor.id FROM authorized i CROSS JOIN LATERAL unnest(i.path) ancestor(id)
   WHERE $21 AND i.fits AND i.kind IN ('org','project','topic','thread','document','chat')
     AND (i.kind IN ('document','chat') OR ancestor.id::text <> i.id)
), reachable AS MATERIALIZED (
  SELECT DISTINCT jsonb_array_elements_text(__GRANTS__->'reachable')::uuid id
  UNION
  SELECT DISTINCT unnest(topic.path)
    FROM nodes topic
   WHERE topic.kind = 'topic' AND topic.deleted_at IS NULL
     AND NOT COALESCE((__GRANTS__->'objects'->topic.id::text->>'view')::boolean, false) AND EXISTS (
     SELECT 1 FROM node_rows child WHERE child.parent_id = topic.id::text
       AND (COALESCE((child."grant"->>'view')::boolean, false) OR COALESCE((child."grant"->'chat'->>'view')::boolean, false))
   )
), focus_paths AS MATERIALIZED (
  SELECT DISTINCT unnest(i.path) id FROM authorized i
   WHERE COALESCE((i."grant"->>'participating')::boolean,false)
      OR COALESCE((i."grant"->'chat'->>'participating')::boolean,false)
), admin_paths AS MATERIALIZED (
  SELECT DISTINCT unnest(i.path) id FROM authorized i
   WHERE $23 IS TRUE AND i."grant"->'admin_access' IS NOT NULL AND i."grant"->'admin_access' <> 'null'::jsonb
), kept_path AS MATERIALIZED (
  -- A locator keeps only a path its reader may already learn through current content.
  SELECT DISTINCT unnest(i.path) id FROM authorized i
   WHERE $40::uuid IS NOT NULL AND COALESCE((i."grant"->>'view')::boolean,false)
     AND i.kind IN ('org','project','topic','thread','document','chat')
     AND (i.id=$40::text OR i.kind IN ('document','chat') AND i.parent_id=$40::text)
), hidden_topics AS MATERIALIZED (
  -- Done topics retain exactly the work that makes their tree row need attention.
  SELECT n.id FROM node_access n JOIN topics topic ON topic.node_id=n.id
   WHERE $39::boolean AND topic.status='done' AND n.deleted_at IS NULL
     AND COALESCE((n."grant"->>'view')::boolean,false)
     AND NOT EXISTS (SELECT 1 FROM kept_path current WHERE current.id=n.id)
     AND NOT EXISTS (
       SELECT 1 FROM node_access child JOIN threads th ON th.node_id=child.id
       LEFT JOIN thread_reads seen ON seen.thread_id=th.node_id AND seen.user_id=$1 AND seen.continuation_id=th.active_continuation_id
       WHERE n.id=ANY(child.path) AND child.archived_at IS NULL
         AND COALESCE((child."grant"->>'view')::boolean,false)
         AND (EXISTS (SELECT 1 FROM approvals a WHERE a.thread_id=child.id AND a.state='pending')
           OR th.activity_seq>COALESCE(seen.last_read_seq,0) AND (seen.thread_id IS NOT NULL OR COALESCE((
             SELECT s.level<>'off' FROM subscriptions s WHERE s.user_id=$1 AND s.node_id=ANY(child.path)
             ORDER BY array_position(child.path,s.node_id) DESC LIMIT 1),false))))
     AND NOT EXISTS (
       SELECT 1 FROM node_access child JOIN chats c ON c.node_id=child.id JOIN chat_states cs ON cs.chat_id=c.id AND cs.user_id=$1
       LEFT JOIN notification_prefs prefs ON prefs.user_id=cs.user_id
       WHERE n.id=ANY(child.path) AND COALESCE((child."grant"->'chat'->>'view')::boolean,false)
         AND (cs.mentions_unread>0 OR c.last_message_seq>cs.last_read_seq AND __CHAT_FOLLOW__<>'off'))
), candidates AS NOT MATERIALIZED (
  SELECT i.* FROM authorized i
   WHERE (NOT $22 OR COALESCE((i."grant"->>'participating')::boolean,false)
        OR ($20 AND i.kind IN ('org','project','topic','thread') AND EXISTS (SELECT 1 FROM focus_paths p WHERE p.id::text=i.id)))
     AND (NOT $35::boolean OR COALESCE((i."grant"->>'view')::boolean,false))
     AND ($36::text IS NULL OR COALESCE((i."grant"->>'view')::boolean,false)
       AND CASE $36 WHEN 'opened' THEN EXISTS (SELECT 1 FROM recents visit WHERE visit.user_id=$1 AND visit.kind=i.kind AND visit.id=i.id::uuid)
         WHEN 'active' THEN i.kind IN ('thread','document') END)
     AND (NOT $37::boolean OR i.kind IN ('org','project','topic','thread','document','chat')
       AND ('thread'=ANY($8::text[]) OR EXISTS (SELECT 1 WHERE cardinality($8::text[])=0)
         OR NOT EXISTS (SELECT 1 FROM nodes ancestor WHERE ancestor.id=ANY(i.path) AND ancestor.kind='thread')))
     AND ($38::boolean OR NOT EXISTS (SELECT 1 FROM nodes ancestor WHERE ancestor.id=ANY(i.path) AND ancestor.archived_at IS NOT NULL))
     AND (NOT $39::boolean OR NOT EXISTS (SELECT 1 FROM hidden_topics hidden WHERE hidden.id=ANY(i.path)))
     AND ($23::boolean IS NULL
       OR $23 = (i."grant"->'admin_access' IS NOT NULL AND i."grant"->'admin_access' <> 'null'::jsonb)
       OR ($23 AND $20 AND i.kind IN ('org','project','topic','thread') AND EXISTS (SELECT 1 FROM admin_paths p WHERE p.id::text=i.id)))
     AND (COALESCE((i."grant"->>'listed')::boolean, (i."grant"->>'view')::boolean, false)
      OR ($20 AND i.kind = 'thread' AND COALESCE((i."grant"->'chat'->>'view')::boolean,false))
      OR ($20 AND i.kind IN ('org','project','topic','thread') AND
          ($6 OR EXISTS (SELECT 1 FROM reachable r WHERE r.id::text = i.id))))
), selected AS (
  SELECT i.*, CASE WHEN $20 AND i.kind IN ('org','project','topic','thread') THEN EXISTS (
      SELECT 1 FROM candidates child WHERE child.parent_id = i.id
        AND (NOT $21 OR child.fits OR (child.kind IN ('org','project','topic','thread')
          AND child.id IN (SELECT id::text FROM filter_paths)))
    ) ELSE false END has_children
    FROM candidates i
   WHERE (NOT $20 AND COALESCE((i."grant"->>'listed')::boolean, (i."grant"->>'view')::boolean, false) AND __MATCH_NAME__
     AND CASE WHEN $5::text IS NULL THEN true WHEN $5 = 'resources' THEN i.parent_id = 'resources'
       WHEN $5 = 'shared' THEN NOT (i.org_id = ANY($3::uuid[])) ELSE ($36::text IS NULL OR i.id<>$5) AND (i.path @> ARRAY[$5::uuid]
         OR $37::boolean AND i.kind='chat' AND i.org_id=$5::uuid
           AND EXISTS (SELECT 1 FROM chats c WHERE c.id=i.id::uuid AND c.kind='group' AND c.node_id IS NULL)) END)
      OR ($20 AND CASE
        WHEN $5::text IS NULL THEN i.kind = 'org' AND i.parent_id IS NULL AND ($6 OR i.org_id = ANY($3::uuid[]))
        WHEN $5 = 'shared' THEN i.kind = 'org' AND i.parent_id IS NULL AND NOT (i.org_id = ANY($3::uuid[]))
        ELSE i.parent_id = $5 END)
), filtered AS (
  -- Only nodes can be ancestors of a matching item. A filtered-out leaf must not force the whole path set.
  SELECT * FROM selected i WHERE ($20 AND NOT $21)
    OR i.fits OR ($20 AND i.kind IN ('org','project','topic','thread')
      AND i.id IN (SELECT id::text FROM filter_paths))
), groups AS (
  SELECT 'group'::text kind, g.id, g.id name, NULL::text parent_id, '{}'::uuid[] path, NULL::uuid org_id,
         NULL::uuid created_by, NULL::uuid owner_id, NULL::timestamptz created_at, NULL::timestamptz last_activity,
         NULL::text state, NULL::bigint size_bytes, NULL::jsonb takes,
         '{"view":false,"named":true,"access":"path_only","share":false,"settings":false}'::jsonb AS "grant",
         false fits,
         CASE WHEN g.id = 'resources' THEN EXISTS (SELECT 1 FROM candidates c WHERE parent_id = 'resources'
                AND (NOT $21 OR c.fits))
              ELSE EXISTS (SELECT 1 FROM candidates c WHERE kind = 'org' AND parent_id IS NULL AND NOT (org_id = ANY($3::uuid[]))
                AND (NOT $21 OR c.fits OR c.id IN (SELECT id::text FROM filter_paths))) END has_children
    FROM (VALUES ('resources'), ('shared')) g(id)
   WHERE $5::text IS NULL AND $20 AND (NOT $6 OR g.id <> 'shared')
), keyed AS (
  SELECT i.*, CASE WHEN $36::text IS NULL THEN __RANK_NAME__ ELSE 0 END AS relevance, $26::timestamptz AS score_at,
  CASE WHEN $20 OR $36::text IS NOT NULL THEN 0::double precision ELSE __VISIT_SCORE__ END AS visit_score,
  CASE WHEN NOT $20 AND $36::text IS NULL AND COALESCE((i."grant"->>'view')::boolean,false) THEN i.last_activity END AS search_activity,
  CASE WHEN COALESCE((i."grant"->>'view')::boolean,false) THEN r.visited_at END AS last_opened,
  CASE WHEN i.kind = 'group' THEN '[]'::jsonb ELSE COALESCE((
    SELECT jsonb_agg(jsonb_build_object('id', tag.id, 'name', tag.name, 'color', tag.color, 'node',
      jsonb_build_object('id', place.id, 'kind', place.kind,
        'name', CASE WHEN COALESCE((__GRANTS__->'objects'->place.id::text->>'named')::boolean,false) THEN place.name END,
        'redacted', NOT COALESCE((__GRANTS__->'objects'->place.id::text->>'named')::boolean,false))) ORDER BY tag.name, tag.id)
      FROM node_tags tt JOIN tags tag ON tag.id = tt.tag_id JOIN nodes place ON place.id = tag.node_id
     WHERE tt.node_id = CASE WHEN i.kind IN ('org','project','topic','thread') THEN i.id::uuid END
       AND COALESCE((i."grant"->>'view')::boolean, false)
  ), '[]'::jsonb) END tags,
  CASE WHEN COALESCE((i."grant"->>'view')::boolean, false) OR i.kind = 'group' THEN CASE $12
    WHEN 'name' THEN lower(i.name)
    WHEN 'kind' THEN i.kind
    WHEN 'created' THEN to_char(i.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS.US')
    WHEN 'last_activity' THEN to_char(i.last_activity AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS.US')
    WHEN 'opened' THEN to_char(r.visited_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS.US')
    WHEN 'where' THEN (SELECT string_agg(CASE WHEN COALESCE((__GRANTS__->'objects'->n.id::text->>'named')::boolean,false) THEN lower(n.name) ELSE '…' END,' / ' ORDER BY array_position(i.path,n.id)) FROM nodes n WHERE n.id=ANY(i.path) AND n.id::text<>i.id)
    WHEN 'size' THEN lpad(i.size_bytes::text, 20, '0')
    -- Nonnegative IEEE bytes preserve numeric load order without rounding cursor boundaries.
    WHEN 'load' THEN encode(float8send((__GRANTS__->'host_loads'->>i.id)::double precision),'hex')
    WHEN 'use' THEN lpad(__GRANTS__->'resource_uses'->>i.id,20,'0')
    WHEN 'running' THEN CASE WHEN i.kind='host' THEN lpad((SELECT count(*) FROM runtimes r WHERE r.host_id=i.id::uuid AND r.state IN ('creating','running') AND r.successor_of IS NULL)::text,20,'0') END
    WHEN 'owner' THEN (SELECT lower(u.display_name) FROM users u WHERE u.id = i.owner_id)
    WHEN 'created_by' THEN (SELECT lower(u.display_name) FROM users u WHERE u.id = i.created_by)
    WHEN 'activity_by' THEN (SELECT lower(u.display_name) FROM documents d JOIN users u ON u.id=d.updated_by WHERE d.id=CASE WHEN i.kind='document' THEN i.id::uuid END)
    WHEN 'state' THEN i.state
    WHEN 'my_access' THEN CASE i."grant"->>'access' WHEN 'view' THEN '1' WHEN 'take_part' THEN '2' WHEN 'manage' THEN '3' END
    WHEN 'tags' THEN (SELECT string_agg(lower(t.name), ',' ORDER BY lower(t.name)) FROM node_tags tt JOIN tags t ON t.id = tt.tag_id WHERE tt.node_id = CASE WHEN i.kind IN ('org','project','topic','thread') THEN i.id::uuid END)
  END ELSE CASE WHEN $12 = 'name' AND (i."grant"->>'named')::boolean THEN lower(i.name) END END sort_key
  FROM (SELECT * FROM filtered UNION ALL SELECT * FROM groups) i
  LEFT JOIN recents r ON (NOT $20 OR $12='opened') AND r.user_id=$1 AND r.kind=i.kind
    AND r.id=CASE WHEN i.kind<>'group' THEN i.id::uuid END
), page AS MATERIALIZED (
SELECT * FROM keyed
 WHERE NOT $14 OR relevance < $25::integer OR (relevance = $25::integer AND (
   visit_score < $27::double precision OR (visit_score = $27::double precision AND (
   ($28::timestamptz IS NOT NULL AND (search_activity IS NULL OR search_activity < $28)) OR
   (search_activity IS NOT DISTINCT FROM $28::timestamptz AND (
   ($15::text IS NOT NULL AND (sort_key IS NULL OR (CASE WHEN $13 THEN sort_key < $15 ELSE sort_key > $15 END))) OR
   (sort_key IS NOT DISTINCT FROM $15::text AND (kind, id) > ($16::text, $17::text))))))))
 ORDER BY relevance DESC, visit_score DESC, search_activity DESC NULLS LAST, CASE WHEN NOT $13 THEN sort_key END ASC NULLS LAST,
          CASE WHEN $13 THEN sort_key END DESC NULLS LAST, kind, id
 LIMIT $18
)
SELECT page.*,
  (SELECT d.updated_by IS DISTINCT FROM $1 AND d.doc_seq>COALESCE(seen.seen_seq,-1)
    FROM documents d LEFT JOIN document_states seen ON seen.document_id=d.id AND seen.user_id=$1
    WHERE d.id=CASE WHEN page.kind='document' THEN page.id::uuid END AND COALESCE((page."grant"->>'view')::boolean,false)) document_unread,
  (SELECT jsonb_build_object('id',u.id,'kind',u.kind,'username',u.username,'display_name',u.display_name,'avatar_color',u.avatar_color)
    FROM documents d JOIN users u ON u.id=d.updated_by WHERE d.id=CASE WHEN page.kind='document' THEN page.id::uuid END
      AND COALESCE((page."grant"->>'view')::boolean,false)) activity_by_brief,
  (SELECT jsonb_build_object('id',u.id,'kind',u.kind,'username',u.username,'display_name',u.display_name,'avatar_color',u.avatar_color)
     FROM users u WHERE u.id = page.owner_id AND COALESCE((page."grant"->>'view')::boolean,false)) owner_brief,
  (SELECT jsonb_build_object('id',u.id,'kind',u.kind,'username',u.username,'display_name',u.display_name,'avatar_color',u.avatar_color)
     FROM users u WHERE u.id = page.created_by AND COALESCE((page."grant"->>'view')::boolean,false)) creator_brief,
  COALESCE((SELECT jsonb_agg(to_jsonb(s)) FROM settings s
    WHERE s.key IN ('sharing.allow_holders','env.needs','env.resources') AND (s.node_id IS NULL OR s.node_id = ANY(page.path))
      AND COALESCE((page."grant"->>'view')::boolean,false)), '[]'::jsonb) sharing_rows,
  (SELECT o.slug FROM orgs o WHERE o.node_id = CASE WHEN page.kind = 'org' THEN page.id::uuid END
     AND page.parent_id IS NULL AND COALESCE((page."grant"->>'view')::boolean,false)) root_address,
  (SELECT jsonb_build_object('id',n.uuid_key,'kind',n.kind,
      'name',CASE WHEN COALESCE((n."grant"->>'named')::boolean,false) THEN n.name END,
      'redacted',NOT COALESCE((n."grant"->>'named')::boolean,false))
    FROM node_rows n WHERE page.kind = 'chat' AND n.uuid_key = page.path[cardinality(page.path)]) path_tail
 FROM page
 ORDER BY relevance DESC, visit_score DESC, search_activity DESC NULLS LAST, CASE WHEN NOT $13 THEN sort_key END ASC NULLS LAST,
          CASE WHEN $13 THEN sort_key END DESC NULLS LAST, kind, id
