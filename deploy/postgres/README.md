# The database image

[Dockerfile](Dockerfile) is the image Compose makes for the `postgres` service: the PostgreSQL image the stack
pins (`postgres:17-alpine`, by its index digest), with the [pgvector](https://github.com/pgvector/pgvector)
extension 0.8.7 added for search by meaning. The extension is built from its source release, checked by its
SHA-256, on that same image, and only its files are copied onto it: about half a megabyte (the library and its
SQL scripts). The server, its settings and the format of the data directory are the image's own, so a data
directory made by the plain image opens unchanged; an index check of an existing table passes after the switch.

Compose makes the image the first time the stack starts (`build: ./postgres`), which needs the base image, the
extension's source and Alpine's build tools; a host without those reaches them through a mirror, or is given the
image made elsewhere (`docker save`/`docker load` of `orch-postgres`). After a change to this file, make it again
with `docker compose build postgres` and restart the service.

Without the extension the stack works as before: the schema's setup says it is absent, and search by meaning
reports that it is unavailable and why, on the admin page. With it, the vectors of each org's index are kept in
`search_vectors` (half precision, with an HNSW index by cosine for each generation).
