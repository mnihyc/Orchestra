CREATE VIEW search_fields AS
 SELECT 'name'::text kind,n.id source_id,n.id part_id,n.id node_id,NULL::uuid block_id,'name'::text field,
        4::int tier,n.name text,GREATEST(n.updated_at,n.activity_at) at,NULL::uuid author_id,NULL::text item_kind,
        false has_file,NULL::jsonb mentions
 FROM nodes n WHERE n.deleted_at IS NULL
 UNION ALL SELECT 'name',n.id,n.id,n.id,NULL,'description',3,n.description,GREATEST(n.updated_at,n.activity_at),NULL,NULL,false,NULL
 FROM nodes n WHERE n.deleted_at IS NULL AND n.description<>''
 UNION ALL SELECT 'document',d.id,d.id,d.node_id,NULL,'title',4,d.title,d.updated_at,d.updated_by,NULL,
        EXISTS(SELECT 1 FROM document_files f WHERE f.document_id=d.id),NULL
 FROM documents d WHERE d.deleted_at IS NULL
 UNION ALL SELECT 'document',d.id,d.id,d.node_id,NULL,'description',3,d.description,d.updated_at,d.updated_by,NULL,
        EXISTS(SELECT 1 FROM document_files f WHERE f.document_id=d.id),NULL
 FROM documents d WHERE d.deleted_at IS NULL AND d.description<>''
 UNION ALL SELECT 'document',d.id,d.id,d.node_id,NULL,'file',2,f.name,f.added_at,f.added_by,NULL,true,NULL
 FROM document_files f JOIN documents d ON d.id=f.document_id WHERE d.deleted_at IS NULL
 UNION ALL SELECT 'document',b.document_id,b.id,d.node_id,b.id,'heading',2,
        b.text,b.updated_at,b.updated_by,NULL,EXISTS(SELECT 1 FROM document_files f WHERE f.document_id=d.id),b.content
 FROM document_blocks b JOIN documents d ON d.id=b.document_id WHERE b.removed_at IS NULL AND d.deleted_at IS NULL AND b.type='heading'
 UNION ALL SELECT 'document',b.document_id,b.id,d.node_id,b.id,'block',0,
        b.text,b.updated_at,b.updated_by,NULL,EXISTS(SELECT 1 FROM document_files f WHERE f.document_id=d.id),b.content
 FROM document_blocks b JOIN documents d ON d.id=b.document_id WHERE b.removed_at IS NULL AND d.deleted_at IS NULL AND b.type<>'heading'
 UNION ALL SELECT 'chat_message',m.chat_id,m.id,COALESCE(c.node_id,c.org_id),NULL,'chat',0,m.search_text,m.created_at,m.author_id,NULL,
        jsonb_path_exists(m.content,'$[*] ? (@.type == "attachment" || @.type == "image" || exists(@.attachments[0]) || exists(@.images[0]))'),
        to_jsonb(m.mentions)
 FROM chat_messages m JOIN chats c ON c.id=m.chat_id WHERE m.deleted_at IS NULL AND m.search_text IS NOT NULL
 UNION ALL SELECT 'item',i.thread_id,i.id,i.thread_id,NULL,'item',0,i.search_text,i.created_at,i.author_id,i.kind,
        jsonb_path_exists(i.payload,'$.parts[*] ? (@.type == "file" || @.type == "image" || exists(@.attachments[0]) || exists(@.images[0]))'),i.payload
 FROM items i WHERE i.search_text IS NOT NULL AND i.kind<>'marker' AND i.search_text NOT LIKE '%<orch>%'
 UNION ALL SELECT 'item',i.thread_id,i.id,i.thread_id,NULL,'revision',0,r.body->>'search_text',i.created_at,i.author_id,i.kind,
        jsonb_path_exists(r.body->'payload','$.parts[*] ? (@.type == "file" || @.type == "image" || exists(@.attachments[0]) || exists(@.images[0]))'),r.body->'payload'
 FROM thread_item_revisions r JOIN items i ON i.id=r.item_id
 WHERE r.body->>'search_text' IS NOT NULL AND i.kind<>'marker' AND r.body->>'search_text' NOT LIKE '%<orch>%';
