-- All monetary values remain text. A point is the last actual reading in a selected chart step,
-- never an average, sum, difference, interpolated endpoint or ledger-derived estimate.
WITH observed AS MATERIALIZED (
    SELECT o.*,
           lag(source_revision) OVER source_order AS previous_source,
           lag(unit) OVER source_order AS previous_unit,
           lag(decimal_places) OVER source_order AS previous_scale,
           row_number() OVER source_order AS observation_number
      FROM provider_credit_observations o
     WHERE provider_id=$1 AND observed_at >= $2 AND observed_at < $3 AND observed_at <= $4
    WINDOW source_order AS (PARTITION BY member,item_id,quantity ORDER BY observed_at,source_revision)
), keyed AS MATERIALIZED (
    SELECT o.*,
           jsonb_build_array(provider_id,source_revision,member,item_id,unit,decimal_places)::text AS group_key,
           jsonb_build_array(provider_id,source_revision,member,item_id,unit,decimal_places,quantity)::text AS key,
           date_trunc($5::text,observed_at,'UTC') AS bin,
           (had_gap OR cleared OR amount IS NULL OR unlimited IS TRUE OR enabled IS FALSE OR
             (observation_number > 1 AND
              (previous_source IS DISTINCT FROM source_revision OR previous_unit IS DISTINCT FROM unit
               OR previous_scale IS DISTINCT FROM decimal_places))) AS gap
      FROM observed o
), groups AS MATERIALIZED (
    SELECT DISTINCT group_key FROM keyed
     WHERE $6::text IS NULL OR group_key > $6
     ORDER BY group_key LIMIT $7
), chosen AS MATERIALIZED (
    SELECT k.* FROM keyed k JOIN groups g USING(group_key)
), buckets AS (
    SELECT c.*,
           row_number() OVER (PARTITION BY key,bin ORDER BY observed_at DESC) AS latest,
           bool_or(gap) OVER (PARTITION BY key,bin) AS gap_before
      FROM chosen c
), points AS (
    SELECT b.*,
           row_number() OVER (PARTITION BY key ORDER BY observed_at) AS point_number,
           count(*) OVER (PARTITION BY key) AS point_count
      FROM buckets b WHERE latest=1
)
SELECT key,group_key,source_revision,member,item_id,quantity,
       (array_agg(label ORDER BY observed_at DESC))[1] AS label,unit,decimal_places,
       jsonb_agg(jsonb_build_object('at',observed_at,'amount',amount,'unlimited',unlimited,
                 'enabled',enabled,'gap_before',gap_before) ORDER BY observed_at)
           FILTER (WHERE point_number <= $8) AS points,
       bool_or(point_count > $8) AS overflow
  FROM points
 GROUP BY key,group_key,source_revision,member,item_id,quantity,unit,decimal_places
 ORDER BY group_key,key
