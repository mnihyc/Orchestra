# Set how access requests and timed access work

Choose the one-click durations offered when someone grants access, how many requests a person may have waiting, and when unanswered requests lapse. At the end people are offered the durations you chose, and requests do not pile up or wait forever.

People ask for and grant access as [Who can see and do what](../user/who-can-see-and-do-what.md#requesting-access) describes.

## Before you start

You need permission to change settings at the system, or at an organization to override the system.

## 1. Choose the settings

| Key | Shipped value | What it does |
|---|---|---|
| `access.quick_grant_durations` | `["1d", "7d", "30d", "always"]` | The one-click grant choices in reference cards, the Inbox and **Approvals**. Each duration is a positive whole number of days; `always` gives an ordinary entry with no deadline. It belongs to the system, an organization can override it, and it can be locked |
| `access.max_pending_requests` | 10 | How many pending batches of requests one person may have |
| `access.request_expiry_days` | 14 | How many days an unanswered access request waits before it lapses |

1. Change the keys at the system, or at an organization.
2. **Other…** in the grant dialog always accepts a date the person chooses. The server works out a timed entry's deadline from its own decision clock.

Only one request from a person can wait on the same thing. Requests to join a group use the same limits and give ordinary chat membership, never a second way to read.

## Timed group membership

A group membership can have a deadline. Access checks enforce it at once. The supervisor then records the removal in `left_at`, adds the normal membership audit, and notifies the current members and the person removed. A direct chat takes no join request.

## Who is told

- A request raised during a turn first tells the person who started the turn, if that person can decide it now.
- Otherwise it goes to the eligible people named directly in the Owners row of the target, then to the eligible owners above it, then to the organization's administrators.
- If nobody in that order can decide, every person who can decide now is told.
- Other people who can decide find the card, or **Others you may decide**.
- A person who loses the authority to decide loses the pending item and its count. A notice sent earlier never grants authority.

Proposed plans and held messages decide through one approval row each, which records the decision and the settlement of its recipients. When you upgrade, existing open ones get that row, and existing approval rows are kept.

## If it does not work

| You see | What to do |
|---|---|
| A person cannot send another request | They have `access.max_pending_requests` batches waiting, or already one request on that thing. Answer or withdraw one, or raise the limit |
| A request disappeared unanswered | It lapsed after `access.request_expiry_days` days. Ask again |
| A grant of a duration you did not list is needed | Use **Other…** and choose a date |
