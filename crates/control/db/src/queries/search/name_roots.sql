WITH held AS MATERIALIZED (
        SELECT * FROM unnest($3::text[],$4::uuid[],$5::text[]) AS given(kind,id,role) WHERE $6
        UNION ALL SELECT * FROM access_holders($1,$2,false) WHERE NOT $6
      ), objects AS (
        SELECT 'node'::text kind,id,id place,org_id,owner_id FROM nodes WHERE deleted_at IS NULL
        UNION ALL SELECT 'document',id,node_id,org_id,owner_id FROM documents WHERE deleted_at IS NULL
        UNION ALL SELECT 'document_container',id,node_id,org_id,owner_id FROM document_containers WHERE deleted_at IS NULL
        UNION ALL SELECT 'chat',id,COALESCE(node_id,home_id,org_id),org_id,NULL FROM chats
        UNION ALL SELECT 'provider',id,home_id,org_id,owner_id FROM providers WHERE deleted_at IS NULL
        UNION ALL SELECT 'agent',id,home_id,org_id,owner_id FROM agents WHERE deleted_at IS NULL
        UNION ALL SELECT 'environment',id,home_node_id,org_id,owner_id FROM environments WHERE deleted_at IS NULL
        UNION ALL SELECT 'host',h.id,h.home_id,h.org_id,h.owner_id FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE m.deleted_at IS NULL
        UNION ALL SELECT 'template',id,home_id,org_id,owner_id FROM env_templates WHERE deleted_at IS NULL
        UNION ALL SELECT 'mcp_server',id,home_id,org_id,owner_id FROM mcp_servers WHERE deleted_at IS NULL
        UNION ALL SELECT 'mount',id,home_id,org_id,owner_id FROM mounts WHERE deleted_at IS NULL
        UNION ALL SELECT 'skill',id,home_id,org_id,owner_id FROM skills WHERE deleted_at IS NULL
        UNION ALL SELECT 'machine',id,home_id,org_id,owner_id FROM machines WHERE deleted_at IS NULL
      ), seeds AS (
        SELECT m.org_id place FROM org_members m WHERE m.user_id=$1
        UNION SELECT n.id FROM held h JOIN nodes n ON n.id=h.id WHERE h.kind IN ('role','visible','reachable')
        UNION SELECT COALESCE(o.place,o.org_id) FROM objects o WHERE o.owner_id=$1
        UNION SELECT COALESCE(o.place,o.org_id) FROM permission_entries e JOIN objects o ON o.kind=e.object_kind AND o.id=e.object_id
          LEFT JOIN nodes n ON n.id=COALESCE(o.place,o.org_id)
          WHERE e.removed_at IS NULL AND (e.expires_at IS NULL OR e.expires_at>$2)
            AND (EXISTS (SELECT 1 FROM held h WHERE
                 (e.holder_kind=h.kind AND e.holder_id=h.id AND e.holder_kind IN ('user','group','members'))
                 OR (e.holder_kind='role' AND h.kind IN ('role','visible') AND h.id=COALESCE(e.holder_id,o.place)
                     AND array_position(ARRAY['viewer','interactor','developer','admin'],h.role)>=array_position(ARRAY['viewer','interactor','developer','admin'],e.holder_role)))
                 OR e.holder_kind='everyone')
        UNION SELECT c.org_id FROM chat_members cm JOIN chats c ON c.id=cm.chat_id WHERE cm.user_id=$1 AND cm.left_at IS NULL
      ), roots AS (
        SELECT DISTINCT n.path[1] root FROM seeds s JOIN nodes n ON n.id=s.place WHERE n.deleted_at IS NULL
      )
      SELECT DISTINCT n.id FROM nodes n JOIN roots r ON n.path[1]=r.root
        WHERE n.kind='org' AND n.deleted_at IS NULL
