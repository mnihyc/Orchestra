-- One bounded window-definition snapshot and its relevant early-reset receipts.
WITH opts AS MATERIALIZED (
    SELECT $1::uuid AS provider,$2::jsonb AS definitions,$3::timestamptz AS from_at,
           $4::timestamptz AS to_at,$5::timestamptz AS now_at,$6::text AS bucket,
           $7::bigint AS max_points,$8::bigint AS max_age_seconds,
           $9::float8 AS reset_drop,$10::bigint AS reset_grace_seconds,$11::bigint AS max_buckets
), requested AS MATERIALIZED (
    SELECT w.*,jsonb_build_object('provider',o.provider,'window',w.window_id) AS labels
      FROM opts o CROSS JOIN jsonb_to_recordset(o.definitions)
           AS w(window_id text,natural_start timestamptz,lookback_seconds bigint)
), receipts AS MATERIALIZED (
    SELECT a.id,a.at,a.action,a.details
      FROM audit_log a CROSS JOIN opts o
     WHERE a.object_kind='provider' AND a.object_id=o.provider
       AND a.action IN ('provider.window_started_over','provider.reset_used')
       AND a.at>=LEAST(o.from_at,COALESCE((SELECT min(natural_start) FROM requested),o.from_at))
       AND a.at<=o.now_at
), resets AS MATERIALIZED (
    SELECT w.window_id,a.at,a.id,a.details->>'cause' AS cause
      FROM receipts a JOIN requested w ON w.window_id=a.details->>'window_id'
     WHERE a.action='provider.window_started_over'
    UNION ALL
    SELECT w.window_id,a.at,a.id,'orch'::text
      FROM receipts a CROSS JOIN opts o
      CROSS JOIN LATERAL (
          SELECT jsonb_object_agg(b->>'id',b) AS by_id
            FROM jsonb_array_elements(CASE WHEN jsonb_typeof(a.details->'windows_before')='array'
                 THEN a.details->'windows_before' ELSE '[]'::jsonb END) b
           WHERE b->>'id' IS NOT NULL
      ) old
      CROSS JOIN LATERAL jsonb_array_elements(CASE WHEN jsonb_typeof(a.details->'windows_after')='array'
           THEN a.details->'windows_after' ELSE '[]'::jsonb END) n
      JOIN requested w ON w.window_id=n->>'id'
     WHERE a.action='provider.reset_used'
       AND (old.by_id->w.window_id->>'used_percent')::float8-(n->>'used_percent')::float8>=o.reset_drop
       AND (old.by_id->w.window_id->>'resets_at')::timestamptz>a.at+make_interval(secs=>o.reset_grace_seconds::float8)
), latest_reset AS MATERIALIZED (
    SELECT DISTINCT ON(window_id) window_id,at,id,cause
      FROM resets ORDER BY window_id,at DESC,id DESC
), windows AS MATERIALIZED (
    SELECT w.*,CASE WHEN w.natural_start IS NOT NULL THEN GREATEST(w.natural_start,r.at) END AS starts_at,
           r.at AS reset_at,r.id AS reset_id,r.cause AS reset_cause
      FROM requested w LEFT JOIN latest_reset r USING(window_id)
)
