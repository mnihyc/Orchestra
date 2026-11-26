-- One connected machine and workspace per org, with ten threads sharing its profile and provider.
-- This extends the same history corpus with current bindings; it starts no process or model call.
INSERT INTO machines(id,name,owner_id,home_id,org_id,placement,roles,platform_os,platform_arch,
                     connector_version,protocol_version,cert_serial,cert_fingerprint,cert_not_after,online,facts)
 SELECT pg_temp.page_id('machine',n),'Scale machine '||n,pg_temp.page_owner(),
        pg_temp.page_id('org',n),pg_temp.page_id('org',n),'connected',ARRAY['agent','provider','environment'],
        'linux','x86_64','0.1.0',1,'page-machine-'||n,decode(md5('page-machine/'||n)||md5('page-machine/'||n),'hex'),
        now()+interval '1 year',true,
        '{"policy":{"agent.harnesses":[{"kind":"fake","path":"/fixture/fake"}],"environment.roots":["/workspace"]}}'::jsonb
 FROM generate_series(1,pg_temp.page_size(10)) n;
INSERT INTO environments(id,name,owner_id,org_id,placement,home_node_id,machine_id,status,generation,workspace_root)
 SELECT pg_temp.page_id('environment',n),'Scale workspace '||n,pg_temp.page_owner(),
        pg_temp.page_id('org',n),'connected',pg_temp.page_id('org',n),pg_temp.page_id('machine',n),'running',1,'/workspace'
 FROM generate_series(1,pg_temp.page_size(10)) n;
UPDATE providers p SET machine_id=pg_temp.page_id('machine',n),key_machine_id=pg_temp.page_id('machine',n),
 status='ready',protocols=ARRAY['openai-responses']
 FROM generate_series(1,pg_temp.page_size(10)) n WHERE p.id=pg_temp.page_id('provider',n);
UPDATE agents a SET machine_id=pg_temp.page_id('machine',n)
 FROM generate_series(1,pg_temp.page_size(10)) n WHERE a.id=pg_temp.page_id('agent',n);
INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,enabled,source,created_by)
 SELECT pg_temp.page_id('model',n),pg_temp.page_id('provider',n),'scale-model','Scale model',
        '{"efforts":[]}'::jsonb,true,'manual',pg_temp.page_owner()
 FROM generate_series(1,pg_temp.page_size(10)) n;
UPDATE threads t SET environment_id=pg_temp.page_id('environment',1+(n-1)/10),
 agent_id=pg_temp.page_id('agent',1+(n-1)/10),provider_id=pg_temp.page_id('provider',1+(n-1)/10),
 arrangement='separated',workspace_dir='work'
 FROM generate_series(1,pg_temp.page_size(100)) n WHERE t.node_id=pg_temp.page_id('thread',n);
INSERT INTO settings(node_id,key,value,updated_by)
 SELECT pg_temp.page_id('thread',n),'agent.model',jsonb_build_object('type','offering','name','scale-model'),pg_temp.page_owner()
 FROM generate_series(1,pg_temp.page_size(100)) n;
INSERT INTO permission_entries(id,object_kind,object_id,page,profile,permission,holder_kind,holder_id,created_by)
 SELECT gen_random_uuid(),kind,pg_temp.page_id(kind,n),'live','any',permission,'members',pg_temp.page_id('org',n),pg_temp.page_owner()
 FROM generate_series(1,pg_temp.page_size(10)) n
 CROSS JOIN (VALUES ('machine','machine.view'),('machine','machine.use'),
                    ('environment','environment.view'),('environment','environment.manage'),
                    ('provider','provider.view'),('provider','provider.use'),
                    ('agent','agent.view'),('agent','agent.use')) grants(kind,permission);
INSERT INTO permission_entries(id,object_kind,object_id,page,profile,permission,holder_kind,holder_id,created_by)
 SELECT gen_random_uuid(),kind,pg_temp.page_id(kind,n),'live','any','access.own','user',pg_temp.page_owner(),pg_temp.page_owner()
 FROM generate_series(1,pg_temp.page_size(10)) n CROSS JOIN unnest(ARRAY['machine','environment']) kind;
-- A connected directory keeps thread interaction private until its owner shares it.
-- These browsers are members reading shared threads, so grant the native permission explicitly.
INSERT INTO permission_entries(id,object_kind,object_id,page,profile,permission,holder_kind,holder_id,created_by)
 SELECT gen_random_uuid(),'node',t.node_id,'live','any','thread.interact','members',n.org_id,pg_temp.page_owner()
 FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.environment_id IS NOT NULL;
ANALYZE;
