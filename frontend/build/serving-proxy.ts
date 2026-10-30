/** Node-side upstreams shared by development serving and built-application preview. */
import type { Buffer } from 'node:buffer';
import { readFileSync } from 'node:fs';
import { Agent } from 'node:https';
import type { ProxyOptions } from 'vite';

type ProxyEnvironment = Readonly<Record<string, string | undefined>>;
type PublicCaReader = (path: string) => Buffer;
type Upstreams = Record<string, ProxyOptions>;

/** No gateway environment means the ordinary API-only configuration, without reading a CA file. */
export function servingProxyConfig(environment: ProxyEnvironment, readPublicCa: PublicCaReader = (path) => readFileSync(path)) {
  const api = environment.ORCH_DEV_API ?? 'http://127.0.0.1:18080';
  const proxy: Upstreams = {
    '/api': { target: api, ws: true },
    '/health': { target: api },
  };
  const address = environment.ORCH_DEV_GATEWAY?.trim();
  if (address !== undefined && address !== '') {
    let gateway: URL;
    try {
      gateway = new URL(address);
    } catch {
      throw new Error('ORCH_DEV_GATEWAY must be an HTTPS origin without userinfo, a path, a query or a fragment.');
    }
    if (gateway.protocol !== 'https:' || gateway.username !== '' || gateway.password !== ''
      || gateway.pathname !== '/' || gateway.search !== '' || gateway.hash !== '') {
      throw new Error('ORCH_DEV_GATEWAY must be an HTTPS origin without userinfo, a path, a query or a fragment.');
    }
    const caPath = environment.ORCH_DEV_GATEWAY_CA?.trim();
    if (caPath === undefined || caPath === '') {
      throw new Error('ORCH_DEV_GATEWAY_CA must name the gateway public CA certificate when ORCH_DEV_GATEWAY is set.');
    }
    let ca: Buffer;
    try {
      ca = readPublicCa(caPath);
    } catch {
      throw new Error('ORCH_DEV_GATEWAY_CA must name a readable public CA certificate.');
    }
    const servername = environment.ORCH_DEV_GATEWAY_SERVERNAME?.trim();
    const agent = new Agent({
      ca,
      rejectUnauthorized: true,
      ...(servername === undefined || servername === '' ? {} : { servername }),
    });
    // Keep the link path and body intact. Trust is local to this upstream, including upgrades;
    // no global TLS settings or client identity/key are needed for the enrollment entrance.
    proxy['/link/'] = { target: gateway.origin, ws: true, secure: true, agent };
  }
  return { server: { proxy }, preview: { proxy } };
}
