# Access requests and timed membership

`access.quick_grant_durations` sets the one-click grant choices in reference cards,
Inbox and Approvals. It belongs to the system with an org override and can be locked.
The shipped list is `["1d", "7d", "30d", "always"]`. Each duration is a positive
whole number of days; `always` gives an ordinary entry with no deadline. The server
computes a timed entry's deadline from its decision clock. Other… accepts a chosen date.

`access.max_pending_requests` limits a person's pending batches (10 by default).
`access.request_expiry_days` lapses unanswered access requests (14 days by default).
Only one request from a person may wait on the same thing. Group join requests use
these same limits and grant ordinary chat membership, never a second read route.

Migration adds source pointers for open proposed plans and held messages. Each uses
one approvals row for its decision and recipient settlement. Existing open sources
receive that row during upgrade; existing approval rows are preserved.

A group membership may have a deadline. Access checks enforce it immediately. The
supervisor then records removal in `left_at`, adds the normal membership audit and
notifies current members and the person removed. A direct chat accepts no join request.

A turn-raised request first tells its starter when that person can currently decide.
Otherwise requests go to eligible people directly named in the target's Owners row,
then eligible owners above it, then org admins. If nobody in that walk can decide,
every currently eligible person is told. Others who can decide find the card or
Others you may decide. Losing deciding authority removes a pending item and its count;
it never grants authority merely because a notice was sent earlier.
