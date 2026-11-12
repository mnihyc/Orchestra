# Organization facts source

A separate optional **Admin key** is stored through the provider's ordinary
encrypted source fields. It is given only to this facts source, with native
`x-api-key` authentication and version/beta headers. It never replaces the model
key or enters a model request or agent environment. Without it, the declared
administrative members are not offered with the reason “needs an admin key”.

The source reads organization identity, the current UTC day's cost report,
configured spend limits and effective per-member limits. Monetary strings stay
exactly in their reported minor units: `USD cents` for USD. It preserves fractional
cents and genuine zero values, without computing a prepaid balance or an aggregate
across overlapping scopes. Only the optional usage-window display computes a
percentage. An unreported reset remains unknown. Effective spend is informational;
the vendor can report zero while a spend reading is temporarily unavailable.

The cost endpoint excludes Priority Tier costs; the source's fallback and localized
descriptions retain that exclusion and the absence of a prepaid balance. Spend-limit
endpoints can require early-access access/scopes; a refusal does not erase successfully read identity or
costs and is not promoted into a failure of the whole source. No unsupported
prepaid balance is declared. A repeated cursor or more than eight pages fails the
bounded read visibly; existing source deadlines, reply and normalized item limits
remain. Current-day daily cost queries normally need one bucket.

Fixture data is authored and anonymized from the published
[cost report](https://platform.claude.com/docs/en/manage-claude/usage-cost-api) and
[spend-limit](https://platform.claude.com/docs/en/manage-claude/spend-limits-api)
shapes. These tests have no live admin credential and make no vendor call.
