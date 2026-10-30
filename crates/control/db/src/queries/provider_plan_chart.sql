-- Aggregate history and preserve current peaks before returning any rows to the application.
, fine AS MATERIALIZED (
    SELECT w.window_id,s.at,s.value
      FROM windows w CROSS JOIN opts o JOIN metric_samples s
        ON s.name='provider_plan_used_percent' AND s.labels=w.labels
       AND s.at>=LEAST(o.from_at,COALESCE(w.starts_at,o.from_at)) AND s.at<=o.now_at
     WHERE s.value>'-Infinity'::float8 AND s.value<'Infinity'::float8
), fine_hours AS MATERIALIZED (
    SELECT DISTINCT window_id,date_bin(interval '1 hour',at,timestamptz '1970-01-01 00:00:00+00') AS hour_at FROM fine
), samples AS MATERIALIZED (
    SELECT window_id,at,value FROM fine
    UNION ALL
    SELECT w.window_id,h.at,h.peak
      FROM windows w CROSS JOIN opts o JOIN metric_hours h
        ON h.name='provider_plan_used_percent' AND h.labels=w.labels
       AND h.at>=LEAST(o.from_at,COALESCE(w.starts_at,o.from_at)) AND h.at<=o.now_at
      LEFT JOIN fine_hours f ON f.window_id=w.window_id AND f.hour_at=h.at
     WHERE f.window_id IS NULL AND h.peak>'-Infinity'::float8 AND h.peak<'Infinity'::float8
), current_bins AS (
    SELECT s.*,floor(extract(epoch FROM(s.at-w.starts_at))*1000 /
           GREATEST(ceil(extract(epoch FROM(o.now_at-w.starts_at))*1000/(o.max_points-1)),1)) AS bin
      FROM samples s JOIN windows w USING(window_id) CROSS JOIN opts o
     WHERE w.starts_at IS NOT NULL AND s.at>=w.starts_at
), current_peaks AS (
    SELECT *,row_number() OVER(PARTITION BY window_id,bin ORDER BY value DESC,at DESC) AS pick FROM current_bins
), current_last AS (
    SELECT DISTINCT ON(window_id) window_id,at,value FROM current_bins ORDER BY window_id,at DESC
), current_kept AS (
    SELECT window_id,at,value FROM current_peaks WHERE pick=1
    UNION
    SELECT window_id,at,value FROM current_last
), current_arrays AS (
    SELECT window_id,jsonb_agg(jsonb_build_object('at',at,'used_percent',value) ORDER BY at) AS points
      FROM current_kept GROUP BY window_id
), history_prior AS (
    SELECT w.window_id,p.at,p.value FROM windows w CROSS JOIN opts o
      JOIN LATERAL (
          SELECT * FROM (
              (SELECT s.at,s.value,1 AS fine FROM metric_samples s
                WHERE s.name='provider_plan_used_percent' AND s.labels=w.labels AND s.at<o.from_at
                  AND s.value>'-Infinity'::float8 AND s.value<'Infinity'::float8 ORDER BY s.at DESC LIMIT 1)
              UNION ALL
              (SELECT h.at,h.peak,0 FROM metric_hours h
                WHERE h.name='provider_plan_used_percent' AND h.labels=w.labels AND h.at<o.from_at
                  AND h.peak>'-Infinity'::float8 AND h.peak<'Infinity'::float8 ORDER BY h.at DESC LIMIT 1)
          ) previous ORDER BY at DESC,fine DESC LIMIT 1
      ) p ON true
), history_samples AS (
    SELECT s.window_id,s.at,s.value FROM samples s CROSS JOIN opts o WHERE s.at>=o.from_at AND s.at<o.to_at
    UNION
    SELECT window_id,at,value FROM history_prior
), history_rises AS (
    SELECT *,GREATEST(value-COALESCE(lag(value) OVER(PARTITION BY window_id ORDER BY at),value),0) AS rise
      FROM history_samples
), history_buckets AS (
    SELECT window_id,date_trunc(o.bucket,at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' AS bin,
           sum(rise)::float8 AS used_percent,bool_or(value>=100) AS reached_limit
      FROM history_rises CROSS JOIN opts o WHERE at>=o.from_at AND at<o.to_at
     GROUP BY window_id,2
), history_numbered AS (
    SELECT *,row_number() OVER(PARTITION BY window_id ORDER BY bin) AS pick,
           count(*) OVER(PARTITION BY window_id) AS buckets FROM history_buckets
), history_arrays AS (
    SELECT window_id,jsonb_agg(jsonb_build_object('at',h.bin,
               'ends_at',(h.bin AT TIME ZONE 'UTC'+CASE o.bucket WHEN 'hour' THEN interval '1 hour'
                   WHEN 'day' THEN interval '1 day' WHEN 'week' THEN interval '1 week' ELSE interval '1 month' END) AT TIME ZONE 'UTC',
               'used_percent',used_percent,'reached_limit',reached_limit) ORDER BY h.bin) FILTER(WHERE h.pick<=o.max_buckets) AS history,
           avg(used_percent)::float8 AS mean_used_percent,count(*) FILTER(WHERE reached_limit) AS limit_buckets,
           bool_or(h.buckets>o.max_buckets) AS overflow
      FROM history_numbered h CROSS JOIN opts o GROUP BY window_id
), reset_bins AS (
    SELECT r.*,date_trunc(o.bucket,r.at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' AS bin
      FROM resets r CROSS JOIN opts o WHERE r.at>=o.from_at AND r.at<o.to_at
), reset_ranked AS (
    SELECT *,count(*) OVER(PARTITION BY window_id,bin) AS count,
           row_number() OVER(PARTITION BY window_id,bin ORDER BY at DESC,id DESC) AS pick FROM reset_bins
), reset_representatives AS (
    SELECT *,row_number() OVER(PARTITION BY window_id ORDER BY at) AS marker FROM reset_ranked WHERE pick=1
), reset_arrays AS (
    SELECT window_id,jsonb_agg(jsonb_build_object('id',id,'at',at,'cause',cause,'count',count) ORDER BY at) FILTER(WHERE marker<=o.max_buckets) AS resets,
           sum(count)::bigint AS early_resets,bool_or(marker>o.max_buckets) AS overflow
      FROM reset_representatives CROSS JOIN opts o GROUP BY window_id
)
SELECT w.window_id,w.starts_at,COALESCE(c.points,'[]'::jsonb) AS points,COALESCE(h.history,'[]'::jsonb) AS history,
       COALESCE(r.resets,'[]'::jsonb) AS resets,
       CASE WHEN w.natural_start IS NOT NULL AND w.reset_at>=w.natural_start THEN
           jsonb_build_object('id',w.reset_id,'at',w.reset_at,'cause',w.reset_cause,'count',1) END AS current_reset,
       jsonb_build_object('mean_used_percent',h.mean_used_percent,'limit_buckets',COALESCE(h.limit_buckets,0),
                         'early_resets',COALESCE(r.early_resets,0)) AS summary,
       COALESCE(h.overflow,false) OR COALESCE(r.overflow,false) AS overflow
  FROM windows w LEFT JOIN current_arrays c USING(window_id) LEFT JOIN history_arrays h USING(window_id)
  LEFT JOIN reset_arrays r USING(window_id) ORDER BY w.window_id
