WITH params AS (
  SELECT $1::text subject,$3::uuid version,$6::timestamptz at,
    $7::uuid after_user,$8::bigint page_limit,$9::boolean recent
), captured AS MATERIALIZED(__SOURCE__), people AS MATERIALIZED(
  SELECT c.id,(u.id IS NOT NULL AND (
    c.id=$4 OR COALESCE(p.audience,'system')='system' OR p.audience='orgs' AND EXISTS(
      SELECT 1 FROM org_members m WHERE m.user_id=c.id AND m.org_id=ANY($5)
    )
  )) visible
  FROM captured c LEFT JOIN users u ON u.id=c.id LEFT JOIN user_profiles p ON p.user_id=c.id
  WHERE c.id IS NOT NULL
)
