#!/usr/bin/env bash
# Exercise the shipped shell installers against local fixture downloads without starting a stack.
set -Eeuo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd -P)"
install_dir="${ORCH_INSTALL_DIR:-$repo/deploy/install}"
image='nginx:1.30-alpine@sha256:0985e772fb9f729e6fa0980da05fca5d9c468e870eed43071545afa9d2e27d94'

docker run -i --rm --pull=never --network none --read-only \
  --tmpfs /work:exec,mode=755,size=16m --tmpfs /tmp:mode=1777,size=16m \
  --tmpfs /root:exec,mode=700,size=16m --tmpfs /usr/local/bin:exec,mode=755,size=16m \
  --tmpfs /etc/orch:mode=755,size=16m --tmpfs /var/lib/orch-node:mode=755,size=16m \
  --tmpfs /var/cache/nginx:mode=755,size=16m \
  -v "$install_dir/install.sh:/input/install.sh:ro" \
  -v "$install_dir/install-node.sh:/input/install-node.sh:ro" \
  --entrypoint /bin/sh "$image" -s <<'INNER'
set -eu
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) printf 'unsupported test architecture\n' >&2; exit 1 ;;
esac
mkdir -p "/work/www/api/v1/downloads/connector/linux" "/work/www/api/v1/downloads/node" /work/bin
# Keep all fixture files in this disposable container until it exits.
cat > /work/bin/rm <<'MOCK'
#!/bin/sh
exit 0
MOCK
chmod 755 /work/bin/rm
PATH="/work/bin:/usr/bin:/bin"
export PATH
cat > "/work/www/api/v1/downloads/connector/linux/$arch" <<'CONNECTOR'
#!/bin/sh
printf '%s\n' "$*" >> /work/connector-calls
exit 0
CONNECTOR
cat > "/work/www/api/v1/downloads/node/$arch" <<'NODE'
#!/bin/sh
printf '%s\n' "$*" >> /work/node-calls
exit 0
NODE
connector="/work/www/api/v1/downloads/connector/linux/$arch"
node="/work/www/api/v1/downloads/node/$arch"
connector_size=$(wc -c < "$connector" | tr -d ' ')
node_size=$(wc -c < "$node" | tr -d ' ')
connector_sha=$(sha256sum "$connector" | cut -d ' ' -f 1)
node_sha=$(sha256sum "$node" | cut -d ' ' -f 1)
connector_blake=unused
node_blake=unused
if command -v b3sum > /dev/null 2>&1; then
  connector_blake=$(b3sum --no-names "$connector")
  node_blake=$(b3sum --no-names "$node")
fi
manifest=/work/www/api/v1/downloads/version.json
printf '{"project":"orch","version":"1.0.0","assets":[{"download_url":"/api/v1/downloads/connector/linux/%s","size":%s,"sha256":"%s","blake3":"%s"},{"download_url":"/api/v1/downloads/node/%s","size":%s,"sha256":"%s","blake3":"%s"}]}\n' \
  "$arch" "$connector_size" "$connector_sha" "$connector_blake" "$arch" "$node_size" "$node_sha" "$node_blake" > "$manifest"
cat > /work/nginx.conf <<'NGINX'
pid /work/nginx.pid;
events { worker_connections 16; }
http {
  access_log off;
  server { listen 127.0.0.1:41727; root /work/www; }
}
NGINX
/usr/sbin/nginx -c /work/nginx.conf -g 'daemon off;' &
if ! curl -fs --retry 5 --retry-connrefused --retry-delay 1 http://127.0.0.1:41727/api/v1/downloads/version.json > /dev/null; then
  printf 'the local fixture server did not start\n' >&2
  exit 1
fi
if ! sh /input/install.sh --url http://127.0.0.1:41727 --token fixture --no-service > /work/connector.log 2>&1; then
  cat /work/connector.log >&2
  exit 1
fi
test -x /root/.local/bin/orch-connector
grep -q 'enroll --url http://127.0.0.1:41727 --token fixture' /work/connector-calls
grep -q '__record-installation' /work/connector-calls
printf 'PASS: the connector installer verifies the system download and enrolls\n'
if ! sh /input/install-node.sh --from http://127.0.0.1:41727 --url wss://gateway.invalid:9443 --token fixture --no-service > /work/node.log 2>&1; then
  cat /work/node.log >&2
  exit 1
fi
test -x /usr/local/bin/orch-node
grep -q 'enroll --url wss://gateway.invalid:9443' /work/node-calls
grep -q '__record-installation' /work/node-calls
printf 'PASS: the node installer verifies the system download and enrolls\n'
# A bad hash must leave the installed program alone and never reach enrollment.
sed "s/$connector_sha/0000000000000000000000000000000000000000000000000000000000000000/" "$manifest" > /work/bad-manifest
cp /work/bad-manifest "$manifest"
if sh /input/install.sh --url http://127.0.0.1:41727 --token fixture --no-service > /work/bad.log 2>&1; then
  printf 'a connector with the wrong hash was accepted\n' >&2
  exit 1
fi
grep -q 'does not have the SHA-256' /work/bad.log
test "$(sha256sum /root/.local/bin/orch-connector | cut -d ' ' -f 1)" = "$connector_sha"
test "$(wc -l < /work/connector-calls)" -eq 2
printf 'PASS: a wrong connector checksum is refused before enrollment\n'
# BusyBox wget cannot enforce the installers' redirect rule. Check the error
# before any download with curl absent from PATH.
mkdir -p /work/busybin
for tool in awk cat grep id mktemp sed sha256sum uname wget; do ln -s /bin/busybox "/work/busybin/$tool"; done
ln -s /work/bin/rm /work/busybin/rm
if PATH=/work/busybin LC_ALL=C /bin/sh /input/install.sh --url http://127.0.0.1:41727 --token fixture --no-service > /work/wget-connector.log 2>&1; then
  printf 'unsupported wget was accepted for the connector\n' >&2
  exit 1
fi
if ! grep -q 'wget with --max-redirect' /work/wget-connector.log; then
  cat /work/wget-connector.log >&2
  exit 1
fi
if PATH=/work/busybin LC_ALL=C /bin/sh /input/install-node.sh --from http://127.0.0.1:41727 --url wss://gateway.invalid:9443 --token fixture --no-service > /work/wget-node.log 2>&1; then
  printf 'unsupported wget was accepted for the node\n' >&2
  exit 1
fi
if ! grep -q 'wget with --max-redirect' /work/wget-node.log; then
  cat /work/wget-node.log >&2
  exit 1
fi
if PATH=/work/busybin LC_ALL=zh_CN /bin/sh /input/install.sh --url http://127.0.0.1:41727 --token fixture --no-service > /work/wget-zh.log 2>&1; then
  printf 'unsupported wget was accepted in Chinese\n' >&2
  exit 1
fi
grep -q '需要 curl 或支持 --max-redirect 的 wget' /work/wget-zh.log
printf 'PASS: unsupported wget is identified before either installer downloads\n'
INNER
