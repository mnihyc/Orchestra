# Orch's database: the PostgreSQL image the stack pins, with vector and short lexical search indexes.
# The extension is built from its pinned source, checked by its SHA-256, on that same image, and only its files
# are copied onto it: the server, its settings and the format of the data directory stay the image's own, so a
# data directory made by the plain image opens unchanged.
ARG POSTGRES_IMAGE=postgres:17-alpine@sha256:b0f9560a2de083e2cc7382e75f808c7381a32852a7ec49117deedb300e552b24

FROM ${POSTGRES_IMAGE} AS build
ADD https://github.com/pgvector/pgvector/archive/refs/tags/v0.8.7.tar.gz /tmp/pgvector.tar.gz
# OPTFLAGS="" builds for any processor of the architecture, not only the one that builds; no LLVM bitcode, so
# the build needs no compiler of the server's own LLVM.
RUN printf '%s  %s\n' cac0b10c360f05b2d521200105ba3697e773d4cd3731f5a915a7e37ebe0bea85 /tmp/pgvector.tar.gz | sha256sum -c - \
 && apk add --no-cache --virtual .build build-base \
 && mkdir /tmp/pgvector \
 && tar -xzf /tmp/pgvector.tar.gz -C /tmp/pgvector --strip-components=1 \
 && make -C /tmp/pgvector OPTFLAGS="" with_llvm=no \
 && make -C /tmp/pgvector install OPTFLAGS="" with_llvm=no DESTDIR=/tmp/out

ADD https://github.com/pgbigm/pg_bigm/archive/refs/tags/v1.2-20250903.tar.gz /tmp/pg_bigm.tar.gz
RUN printf '%s  %s\n' 4d4fb48161e3f9e99207851299379c83b3e5d1d3c1a07c35f6a3153d577b3142 /tmp/pg_bigm.tar.gz | sha256sum -c - \
 && apk add --no-cache --virtual .bigm-build icu-dev \
 && mkdir /tmp/pg_bigm \
 && tar -xzf /tmp/pg_bigm.tar.gz -C /tmp/pg_bigm --strip-components=1 \
 && make -C /tmp/pg_bigm USE_PGXS=1 with_llvm=no \
 && make -C /tmp/pg_bigm USE_PGXS=1 with_llvm=no install DESTDIR=/tmp/out

FROM ${POSTGRES_IMAGE}
COPY --from=build /tmp/out/usr/local/lib/postgresql/vector.so /usr/local/lib/postgresql/
COPY --from=build /tmp/out/usr/local/lib/postgresql/pg_bigm.so /usr/local/lib/postgresql/
COPY --from=build /tmp/out/usr/local/share/postgresql/extension/ /usr/local/share/postgresql/extension/
LABEL org.opencontainers.image.title="Orch database" \
      org.opencontainers.image.description="PostgreSQL with vector and bigram search indexes" \
      org.opencontainers.image.licenses="PostgreSQL" \
      orch.image.pgvector="0.8.7" \
      orch.image.pg_bigm="1.2-20250903"
