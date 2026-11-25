# update scenario

An installed release advances to a newer one. First a release whose schema cannot be applied is installed: it
must fail to become ready, and the previous payloads, the database as the dump holds it (the users table is
back), the link records and the installed release must all be as they were. Then the real update: the stack
reports the new release, every service too, the administrator still signs in, the previous release is kept as
`previous`, a dump of the database as the old release left it is kept and reads back, and the configuration,
the secrets, `.env` and `compose.yml` (which the release did not change) are byte for byte what they were. The
earlier release is made to hold the `nginx.conf` of v0.1.1, without the routes the keys need, and the later a newer
generation of the deployment files with an updater of its own (`scripts/smoke/deploy_variant.py`): the update brings
the release's `nginx.conf`, so the edge hands `/openapi/` to the API, and its `update.sh`, replaced as the last act;
the failed update puts the earlier files back and leaves the updater; a rollback puts the earlier `nginx.conf` back and
leaves the newer updater. An `nginx.conf` edited here keeps the set, and an update to the release that asks for the
newer files stops before it stops anything (no container is touched, no journal is opened), names the `.new` file to
merge and, once it is merged and removed, goes on. An update to what is installed does nothing, and a backup stops
nothing.
