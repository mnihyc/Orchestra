// @vitest-environment node
/** Serving a built application and serving its sources agree on optional verified gateway routing. */
import { Buffer } from 'node:buffer';
import type { AgentOptions } from 'node:https';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { servingProxyConfig } from '../../build/serving-proxy';

const { agents, instances } = vi.hoisted(() => {
  const agents: AgentOptions[] = [];
  const instances: object[] = [];
  return { agents, instances };
});
vi.mock('node:https', () => ({
  Agent: function Agent(this: object, options: AgentOptions) {
    agents.push(options); instances.push(this);
  },
}));
const publicCa = Buffer.from('synthetic public CA input; no certificate or key is read by these tests');
const gateway = {
  ORCH_DEV_GATEWAY: 'https://127.0.0.1:9444/',
  ORCH_DEV_GATEWAY_CA: '/fixture/public-ca.pem',
  ORCH_DEV_GATEWAY_SERVERNAME: 'gateway',
};
beforeEach(() => { agents.length = 0; instances.length = 0; });

describe('serving upstream configuration', () => {
  it('retains both ordinary API routes and their upgrade behavior without creating a gateway agent or reading trust', () => {
    const read = vi.fn<(path: string) => Buffer>();
    const config = servingProxyConfig({}, read);
    for (const mode of [config.server, config.preview]) {
      expect(mode.proxy).toStrictEqual({
        '/api': { target: 'http://127.0.0.1:18080', ws: true },
        '/health': { target: 'http://127.0.0.1:18080' },
      });
    }
    expect(read).not.toHaveBeenCalled(); expect(agents).toEqual([]);
  });

  it.each([undefined, '', '   '])('does not activate from CA/server-name settings alone (gateway: %s)', (address) => {
    const read = vi.fn<(path: string) => Buffer>();
    const config = servingProxyConfig({ ...gateway, ORCH_DEV_GATEWAY: address }, read);
    expect(config.server.proxy).not.toHaveProperty('/link/');
    expect(config.preview.proxy).not.toHaveProperty('/link/');
    expect(read).not.toHaveBeenCalled(); expect(agents).toEqual([]);
  });

  it('uses the same link route and explicit verified CA/server name in dev and preview while preserving the API target', () => {
    const read = vi.fn<(path: string) => Buffer>().mockReturnValue(publicCa);
    const config = servingProxyConfig({ ...gateway, ORCH_DEV_API: 'http://api.example.test:8080' }, read);
    expect(config.server.proxy).toBe(config.preview.proxy);
    expect(config.server.proxy['/link/']).toMatchObject({ target: 'https://127.0.0.1:9444', ws: true, secure: true });
    expect(instances).toHaveLength(1);
    expect(config.server.proxy['/link/']).toHaveProperty('agent', instances[0]);
    expect(config.server.proxy['/link/']).not.toHaveProperty('rewrite');
    expect(config.server.proxy['/link/']).not.toHaveProperty('ignorePath');
    expect(config.server.proxy['/link/']).not.toHaveProperty('headers');
    expect(config.server.proxy['/api']).toEqual({ target: 'http://api.example.test:8080', ws: true });
    expect(config.server.proxy['/health']).toEqual({ target: 'http://api.example.test:8080' });
    expect(read).toHaveBeenCalledExactlyOnceWith('/fixture/public-ca.pem');
    expect(agents).toStrictEqual([{ ca: publicCa, rejectUnauthorized: true, servername: 'gateway' }]);
  });

  it('retains the gateway hostname verification default when no TLS server name is supplied', () => {
    const read = vi.fn<(path: string) => Buffer>().mockReturnValue(publicCa);
    const config = servingProxyConfig({ ...gateway, ORCH_DEV_GATEWAY: 'https://gateway.example.test:9444', ORCH_DEV_GATEWAY_SERVERNAME: undefined }, read);
    expect(config.preview.proxy['/link/']).toMatchObject({ target: 'https://gateway.example.test:9444', secure: true });
    expect(agents).toStrictEqual([{ ca: publicCa, rejectUnauthorized: true }]);
  });

  it('keeps a separately configured API address unchanged without a gateway', () => {
    const read = vi.fn<(path: string) => Buffer>();
    const config = servingProxyConfig({ ORCH_DEV_API: 'https://api.example.test:8443' }, read);
    expect(config.preview.proxy['/api']).toEqual({ target: 'https://api.example.test:8443', ws: true });
    expect(config.server.proxy['/health']).toEqual({ target: 'https://api.example.test:8443' });
    expect(read).not.toHaveBeenCalled(); expect(agents).toEqual([]);
  });

  it.each([
    'not-an-address', 'http://127.0.0.1:9444', 'wss://gateway.example.test:9444',
    'https://gateway.example.test/prefix', 'https://gateway.example.test/?query=1',
    'https://gateway.example.test/#fragment', 'https://name@gateway.example.test',
  ])('refuses an unusable gateway origin before reading a file: %s', (address) => {
    const read = vi.fn<(path: string) => Buffer>();
    expect(() => servingProxyConfig({ ...gateway, ORCH_DEV_GATEWAY: address }, read)).toThrow('ORCH_DEV_GATEWAY must be an HTTPS origin');
    expect(read).not.toHaveBeenCalled(); expect(agents).toEqual([]);
  });

  it.each([undefined, '', '  '])('requires explicit gateway public trust (CA: %s)', (ca) => {
    const read = vi.fn<(path: string) => Buffer>();
    expect(() => servingProxyConfig({ ...gateway, ORCH_DEV_GATEWAY_CA: ca }, read)).toThrow('ORCH_DEV_GATEWAY_CA must name the gateway public CA certificate');
    expect(read).not.toHaveBeenCalled(); expect(agents).toEqual([]);
  });

  it('fails configuration when the public CA cannot be read rather than falling back to unverified TLS', () => {
    const read = vi.fn<(path: string) => Buffer>().mockImplementation(() => { throw new Error('fixture read failed'); });
    expect(() => servingProxyConfig(gateway, read)).toThrow('ORCH_DEV_GATEWAY_CA must name a readable public CA certificate.');
    expect(read).toHaveBeenCalledExactlyOnceWith('/fixture/public-ca.pem');
    expect(agents).toEqual([]);
  });
});
