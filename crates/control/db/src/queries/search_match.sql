-- The direct predicate exposes one selective index condition; the array predicate keeps every literal term.
(cardinality({patterns}::text[])=0 OR
 ({text} ILIKE ({patterns}::text[])[1] AND {text} ILIKE ALL({patterns}::text[])))
