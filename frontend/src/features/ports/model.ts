/** Pure presentation rules for listener targets, acknowledged addresses, expiry and host transports. */
import type { EnvironmentPort, EnvironmentPorts, HostTransports, PortOpening } from './presentation';

/** A reviewed host address is live only after its bind acknowledgement. */
export function liveAddress(opening: PortOpening): string | null {
  return opening.way === 'host' && (opening.host_allocated === true || opening.host_allocated === undefined && opening.state === 'open') && opening.host_port !== null ? opening.url : null;
}
export function outsideRange(opening: PortOpening, declaration: EnvironmentPorts['host_ports']): boolean {
  return opening.way === 'host' && opening.host_port !== null && declaration !== null && !declaration.ranges.some((range) => opening.host_port !== null && opening.host_port >= range.from && opening.host_port <= range.to);
}
/** No guessed expiry and no infinite link; the current server maximum bounds the date. */
export function validExpiry(value: string, maxSeconds: unknown, now: number): boolean {
  const at = Date.parse(value);
  return Number.isFinite(at) && at > now && (typeof maxSeconds !== 'number' || at <= now + maxSeconds * 1_000);
}
export function reachedAddress(host: string, port: number): string { return `${host.includes(':') && !host.startsWith('[') ? `[${host}]` : host}:${port}`; }
export const DEFAULT_HOST_TRANSPORTS: HostTransports = 'tcp_udp';
/** Native discovery and live connection reads use the process observation cadence while visible. */
export const PORTS_POLL_MS = 5_000;

/** Converts only an environment-local URL using its served stable address; knowing a name grants no access. */
export function portAddressFor(local: string, ports: readonly Pick<EnvironmentPort, 'port' | 'url'>[], environmentId?: string): string | null {
  let source: URL;
  const explicit = /^(?:(?:http|https|tcp|udp):\/\/)?(?:localhost|127(?:\.\d{1,3}){3}|0\.0\.0\.0|\[(?:::1|::)\]):(\d+)(?:[/?#]|$)/i.exec(local);
  if (explicit === null || Number(explicit[1]) < 1 || Number(explicit[1]) > 65535) return null;
  try { source = new URL(local.includes('://') ? local : `tcp://${local}`); } catch { return null; }
  if (!['http:', 'https:', 'tcp:', 'udp:'].includes(source.protocol) || source.username !== '' || source.password !== '' || !(/^(?:localhost|127(?:\.\d{1,3}){3}|0\.0\.0\.0|\[(?:::1|::)\])$/i.test(source.hostname))) return null;
  const port = Number(explicit[1]);
  const stable = ports.find((row) => row.port === port)?.url;
  if (!['http:', 'https:'].includes(source.protocol) || stable === null || stable === undefined) return environmentId === undefined ? null : `/resources/environments/${encodeURIComponent(environmentId)}?tab=ports&port=${port}${source.protocol === 'udp:' ? '&transport=udp' : ''}`;
  let target: URL;
  try { target = new URL(stable); } catch { return null; }
  if (target.protocol !== 'https:' && target.protocol !== 'http:') return null;
  target.pathname = source.pathname; target.search = source.search; target.hash = source.hash;
  return target.href;
}
