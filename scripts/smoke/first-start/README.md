# first-start scenario

A clean directory and a release become a running system. It first shows that a release with a damaged file is
refused before anything is installed, and that a directory holding a part of the secrets is refused and left as
it was. Then it runs `update.sh first-start --from`, and checks: `/health` and `GET /api/v1/meta` through the
web edge with the release's version; the security headers on every response, the content policy, the cookies
of a sign-in (`HttpOnly`, `Secure`, `SameSite=Strict`); the first administrator and signing in; `orchctl doctor`
in the API's container, then `orchctl doctor --url` from inside the stack (a page, an API call, a signed-in
realtime socket held idle, a body of 8 MiB, the link's upgrade, each advertised endpoint); the bundled node
enrolling once there is an administrator; and that a second first start is refused.
