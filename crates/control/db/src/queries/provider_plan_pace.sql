-- One pace summary per bounded source window, using indexed hour/day look-backs.
, latest AS MATERIALIZED (
    SELECT w.*,p.at AS last_at,p.value AS last_percent FROM windows w CROSS JOIN opts o
      LEFT JOIN LATERAL (
          SELECT * FROM (
              (SELECT s.at,s.value,1 AS fine FROM metric_samples s
                WHERE s.name='provider_plan_used_percent' AND s.labels=w.labels
                  AND s.at>=w.starts_at AND s.at<=o.now_at AND s.value>=0 AND s.value<'Infinity'::float8
                ORDER BY s.at DESC LIMIT 1)
              UNION ALL
              (SELECT h.at,h.peak,0 FROM metric_hours h
                WHERE h.name='provider_plan_used_percent' AND h.labels=w.labels
                  AND h.at>=w.starts_at AND h.at<=o.now_at AND h.peak>=0 AND h.peak<'Infinity'::float8
                ORDER BY h.at DESC LIMIT 1)
          ) observed ORDER BY at DESC,fine DESC LIMIT 1
      ) p ON true
), recent_fine AS MATERIALIZED (
    SELECT w.window_id,s.at,s.value FROM latest w CROSS JOIN opts o JOIN metric_samples s
      ON s.name='provider_plan_used_percent' AND s.labels=w.labels
     AND s.at>=GREATEST(w.starts_at,w.last_at-make_interval(secs=>w.lookback_seconds::float8)) AND s.at<=w.last_at
     WHERE w.last_at>=o.now_at-make_interval(secs=>o.max_age_seconds::float8)
       AND s.value>=0 AND s.value<'Infinity'::float8
), recent_hours AS MATERIALIZED (
    SELECT DISTINCT window_id,date_bin(interval '1 hour',at,timestamptz '1970-01-01 00:00:00+00') AS hour_at FROM recent_fine
), recent AS MATERIALIZED (
    SELECT window_id,at,value FROM recent_fine
    UNION ALL
    SELECT w.window_id,h.at,h.peak FROM latest w CROSS JOIN opts o JOIN metric_hours h
      ON h.name='provider_plan_used_percent' AND h.labels=w.labels
     AND h.at>=GREATEST(w.starts_at,w.last_at-make_interval(secs=>w.lookback_seconds::float8)) AND h.at<=w.last_at
      LEFT JOIN recent_hours f ON f.window_id=w.window_id AND f.hour_at=h.at
     WHERE w.last_at>=o.now_at-make_interval(secs=>o.max_age_seconds::float8)
       AND f.window_id IS NULL AND h.peak>=0 AND h.peak<'Infinity'::float8
), rises AS (
    SELECT *,first_value(value) OVER(PARTITION BY window_id ORDER BY at) AS first_percent,
           GREATEST(value-COALESCE(lag(value) OVER(PARTITION BY window_id ORDER BY at),value),0) AS rise
      FROM recent
), pace AS (
    SELECT window_id,min(at) AS first_at,max(first_percent)::float8 AS first_percent,sum(rise)::float8 AS rise
      FROM rises GROUP BY window_id
)
SELECT w.window_id,w.starts_at,p.first_at,p.first_percent,w.last_at,w.last_percent,COALESCE(p.rise,0) AS rise
  FROM latest w LEFT JOIN pace p USING(window_id) ORDER BY w.window_id
