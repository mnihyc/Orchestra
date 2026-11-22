-- Raw identities only: the next selected frame makes the exact current access decision.
node_access AS NOT MATERIALIZED (
 SELECT n.*,t.state thread_state,t.last_activity_at thread_activity_at,
  '{"view":true,"participating":true,"chat":{"view":true,"participating":true}}'::jsonb AS "grant"
 FROM nodes n LEFT JOIN threads t ON t.node_id=n.id
 WHERE n.deleted_at IS NULL AND NOT n.unlisted AND t.side_of IS NULL
  AND (NOT ($5::jsonb ? 'search_roots') OR n.org_id IN
       (SELECT value::uuid FROM jsonb_array_elements_text($5::jsonb->'search_roots')))
  AND NOT EXISTS(SELECT 1 FROM nodes gone WHERE gone.id=ANY(n.path) AND gone.deleted_at IS NOT NULL)
)
