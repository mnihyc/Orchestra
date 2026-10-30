/** Ports presentation tests: current identities, bound addresses and deliberately bounded link lifetimes. */
import { describe, expect, it } from 'vitest';
import { DEFAULT_HOST_TRANSPORTS, liveAddress, portAddressFor, reachedAddress, validExpiry } from '@/features/ports/model';
import type { PortOpening } from '@/features/ports/presentation';
import { parsePortRanges } from '@/features/ports/HostDeclaration';
import { preferredEnvironmentTab, rememberEnvironmentTab } from '@/features/ports/preference';

describe('a port keeps one address independently of its program', () => {
  it('turns a local app URL into its served stable address and rejects an arbitrary outside target', () => {
    const ports = [{ port: 3000, url: 'https://stable.ports.example' }];
    expect(portAddressFor('http://127.0.0.1:3000/app?q=one#two', ports)).toBe('https://stable.ports.example/app?q=one#two');
    expect(portAddressFor('http://example.com:3000/app', ports)).toBeNull();
    expect(portAddressFor('http://user:password@localhost:3000', ports)).toBeNull();
    expect(portAddressFor('http://localhost:3000', [{ port: 3000, url: null }])).toBeNull();
    expect(DEFAULT_HOST_TRANSPORTS).toBe('tcp_udp');
  });
  it('shows an address for a host only after its acknowledgement', () => {
    const pending = { way: 'host', state: 'opening', host_port: 41003, url: 'host.example:41003' } as PortOpening;
    expect(liveAddress(pending)).toBeNull();
    expect(liveAddress({ ...pending, state: 'failed' })).toBeNull();
    expect(liveAddress({ ...pending, state: 'open' })).toBe('host.example:41003');
    expect(reachedAddress('2001:db8::1', 41003)).toBe('[2001:db8::1]:41003');
  });
  it('requires an expiry and refuses a past date or a zero or exceeded current maximum', () => {
    const now = Date.parse('2026-10-13T09:00:00Z');
    expect(validExpiry('', 86_400, now)).toBe(false);
    expect(validExpiry('2026-10-13T08:00:00Z', 86_400, now)).toBe(false);
    expect(validExpiry('2026-10-14T09:00:01Z', 86_400, now)).toBe(false);
    expect(validExpiry('2026-10-13T10:00:00Z', 0, now)).toBe(false);
    expect(validExpiry('2026-10-13T10:00:00Z', 86_400, now)).toBe(true);
  });
  it('accepts an inclusive sixteen-port proposal and refuses overlapping or reversed ranges', () => {
    expect(parsePortRanges('41000–41015')).toEqual([{ from: 41000, to: 41015 }]);
    expect(parsePortRanges('41000-41015, 41015-41030')).toBeNull();
    expect(parsePortRanges('41015-41000')).toBeNull();
    expect(parsePortRanges('0-16')).toBeNull();
  });
  it('remembers only the chosen view and discards an unknown view', () => {
    rememberEnvironmentTab('one', 'ports');
    expect(preferredEnvironmentTab('one', ['overview', 'ports'])).toBe('ports');
    expect(preferredEnvironmentTab('two', ['overview', 'ports'])).toBeNull();
    rememberEnvironmentTab('one', 'removed-view');
    expect(preferredEnvironmentTab('one', ['overview', 'ports'])).toBeNull();
  });
});


it('links explicit local bare and UDP addresses to one environment target without rewriting the text', () => {
  const original = 'udp://[::1]:5353';
  expect(portAddressFor(original, [], 'environment-1')).toBe('/resources/environments/environment-1?tab=ports&port=5353&transport=udp');
  expect(original).toBe('udp://[::1]:5353');
  expect(portAddressFor('localhost:8080', [], 'environment-1')).toContain('port=8080');
  expect(portAddressFor('https://localhost', [], 'environment-1')).toBeNull();
  expect(portAddressFor('https://example.com:3000', [], 'environment-1')).toBeNull();
});

describe('a local address a shell prints', () => {
  it('leads to the environment port through Orch where it names one, with its exact columns, and leads nowhere without a Ports door', async () => {
    const { localLinks } = await import('@/features/ports/LocalAddresses');
    const rewrite = (href: string) => portAddressFor(href, [{ port: 3000, url: 'https://e1-p3000.ports.example' }], 'environment-1');
    const line = '  ➜  Local:   http://localhost:3000/ and 0.0.0.0:5173, see https://example.org:3000/';
    const found = localLinks(line, rewrite);
    expect(found.map((link) => link.text)).toEqual(['http://localhost:3000/', '0.0.0.0:5173']);
    expect(found[0]).toMatchObject({ start: line.indexOf('http://localhost'), target: 'https://e1-p3000.ports.example/' });
    expect(found[0]!.end - found[0]!.start).toBe('http://localhost:3000/'.length);
    expect(found[1]!.target).toBe('/resources/environments/environment-1?tab=ports&port=5173');
    expect(localLinks(line, null)).toEqual([]);
  });
});
