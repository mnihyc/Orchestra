-- A shared host is not attribution: the actual job must retain this accepted choice and source.
SELECT j.id
FROM pool_choices c JOIN jobs j ON j.plan->>'pool_choice'=c.id::text
WHERE c.state='selected' AND c.path @> jsonb_build_array(jsonb_build_object('pool_id',$1::uuid))
  AND j.state IN ('queued','running') AND NOT (j.plan ? 'committed')
  AND ($2::uuid[] IS NULL OR j.id=ANY($2))
  AND ((j.kind='environment_provision' AND c.source->>'kind'='environment_creation'
        AND c.source->>'environment'=j.environment_id::text)
       OR (j.kind IN ('provider_provision','provider_recreate') AND c.source->>'kind'='provider_placement'
           AND c.id=j.id AND c.source->>'job'=j.id::text
           AND c.source->>'job_kind'=j.kind AND c.source->>'provider'=j.provider_id::text))
ORDER BY j.id
