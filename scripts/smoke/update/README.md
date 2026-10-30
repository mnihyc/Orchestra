# update scenario

An installed release advances to a newer one. First a release whose schema cannot be applied is installed: it
must fail to become ready, and the previous payloads, the database as the dump holds it (the users table is
back), the link records and the installed release must all be as they were. Then the real update: the stack
reports the new release, every service too, the administrator still signs in, the previous release is kept as
`previous`, a dump of the database as the old release left it is kept and reads back, and the configuration,
the secrets, `compose.yml`, `nginx.conf` and `.env` are byte for byte what they were. An update to what is
installed does nothing, and a backup stops nothing.
