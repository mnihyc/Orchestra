/** The shipped edge's TLS negotiation and HTML policy with isolated synthetic certificates. */
import { expect, test } from '@playwright/test';
import { openingTimings } from './opening-timing.ts';
import { execFile } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdir, readFile, rename, symlink, writeFile } from 'node:fs/promises';
import { connect, type IncomingHttpHeaders } from 'node:http2';
import { createServer } from 'node:net';
import { resolve } from 'node:path';
import { TLSSocket } from 'node:tls';
import { promisify } from 'node:util';
import { meta } from '../doubles/fixtures.ts';
import { SETTINGS_REGISTRY } from '../../src/generated/settings-registry.ts';

const execute = promisify(execFile);

async function root(port: number, ca: Buffer, ticket?: Buffer): Promise<{ protocol: string; headers: IncomingHttpHeaders; body: string; ticket?: Buffer; reused: boolean }> {
  return new Promise((accept, reject) => {
    const session = connect(`https://127.0.0.1:${port}`, { ca, servername: 'orch.test', ...(ticket === undefined ? {} : { session: ticket }) });
    let nextTicket: Buffer | undefined;
    const socket = session.socket;
    if (socket instanceof TLSSocket) socket.on('session', (value: Buffer) => { nextTicket = value; });
    session.once('error', (error: Error) => { session.destroy(); reject(error); });
    session.once('connect', () => {
      const protocol = session.alpnProtocol ?? '';
      if (protocol !== 'h2') { session.destroy(); reject(new Error('The stock HTTPS listener did not negotiate h2.')); return; }
      const request = session.request({ ':method': 'GET', ':path': '/' });
      let headers: IncomingHttpHeaders = {}, body = '';
      request.setEncoding('utf8');
      request.on('response', (value) => { headers = value; });
      request.on('data', (value: string) => { body += value; });
      request.once('error', (error: Error) => { session.destroy(); reject(error); });
      request.once('end', () => { const reused = socket instanceof TLSSocket && socket.isSessionReused(); session.close(); accept({ protocol, headers, body, ...(nextTicket === undefined ? {} : { ticket: nextTicket }), reused }); });
      request.end();
    });
  });
}

function transport(overrides: Readonly<Record<string, number | boolean>> = {}): string {
  const defaults = SETTINGS_REGISTRY.find((entry) => entry.key === 'edge.transport')?.default;
  if (typeof defaults !== 'object' || defaults === null || Array.isArray(defaults)) throw new Error('The edge fixture uses the canonical transport default.');
  const values: Record<string, unknown> = { ...defaults, ...overrides };
  const number = (name: string) => { const value = values[name]; if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) throw new Error(`Invalid transport fixture ${name}`); return value; };
  const on = (name: string) => { const value = values[name]; if (typeof value !== 'boolean') throw new Error(`Invalid transport fixture ${name}`); return value ? 'on' : 'off'; };
  const cache = number('session_cache_mib');
  return `# reload_secs=${number('reload_secs')}\nhttp2 ${on('http2')};\nssl_session_cache ${cache === 0 ? 'off' : `shared:orch_edge_${cache}:${cache}m`};\nssl_session_timeout ${number('session_timeout_secs')}s;\nssl_session_tickets ${on('session_tickets')};\nssl_early_data ${on('early_data')};\nhttp3 ${on('http3')};\n${on('http3') === 'on' ? `listen 443 quic reuseport;\nadd_header Alt-Svc 'h3=":$orch_https_port"; ma=${number('alt_svc_secs')}' always;` : "add_header Alt-Svc 'clear' always;"}\n`;
}

async function availablePort(): Promise<number> {
  return new Promise((accept, reject) => {
    const server = createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (address === null || typeof address === 'string') { server.close(); reject(new Error('The isolated edge needs a TCP port.')); return; }
      server.close((error) => { if (error !== undefined) reject(error); else accept(address.port); });
    });
  });
}

test('the release contract refuses an edge that loses resumption, early-write protection or QUIC', async () => {
  const result = await execute('python3', ['-m', 'unittest', 'discover', '-s', '.github/scripts', '-p', 'test_release_contract.py'], { cwd: resolve(process.cwd(), '..') });
  process.stdout.write(result.stdout + result.stderr);
});

test('the stock HTTPS edge negotiates h2 and authorizes each document’s own inline starter', async ({ browser }) => {
  const info = test.info();
  const project = resolve(process.cwd(), '..'), deploy = resolve(project, 'deploy');
  const compose = await readFile(resolve(deploy, 'compose.yml'), 'utf8');
  const image = /^\s+image:\s+(nginx:\S+)\s*$/m.exec(compose)?.[1];
  if (image === undefined) throw new Error('The shipped edge image is required.');
  const directory = info.outputPath('tls'), generation = resolve(directory, 'generation');
  await mkdir(generation, { recursive: true });
  const certificate = resolve(generation, 'chain.pem');
  await execute('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=orch.test',
    '-addext', 'subjectAltName=DNS:orch.test,DNS:api,IP:127.0.0.1', '-keyout', resolve(generation, 'key.pem'), '-out', certificate]);
  await symlink('generation', resolve(directory, 'current'));
  const policyFile = resolve(directory, 'transport.include');
  await writeFile(policyFile, transport());
  // The native fixture accepts only anonymous reads. Use the shipped edge in front of it.
  const anonymous = meta({ name: 'Research & Tools </template>', may_create_org: false });
  const native = `server {
    listen 127.0.0.1:8443 ssl;
    ssl_certificate /etc/orch/frontend-tls/current/chain.pem;
    ssl_certificate_key /etc/orch/frontend-tls/current/key.pem;
    location = /api/v1/auth/session {
      default_type application/json;
      return 200 'null';
    }
    location = /api/v1/meta {
      if ($http_authorization) { return 403; }
      if ($http_cookie) { return 403; }
      default_type application/json;
      return 200 '${JSON.stringify(anonymous)}';
    }
  }`;
  const config = await readFile(resolve(deploy, 'nginx.conf'), 'utf8');
  const fixtureConfig = resolve(directory, 'edge.conf');
  await writeFile(fixtureConfig, config.replace('upstream orch_api {', native + '\n    upstream orch_api {')
    .replace('server api:8443 resolve;', 'server 127.0.0.1:8443;'));
  const port = await availablePort();
  const { stdout } = await execute('docker', ['run', '--detach', '--name', `orch-edge-${randomUUID()}`,
    '--publish', `127.0.0.1:${port}:443/tcp`, '--publish', `127.0.0.1:${port}:443/udp`, '--env', 'ORCH_FRONTEND_HTTPS_BIND=127.0.0.1:443',
    '--volume', `${fixtureConfig}:/etc/nginx/nginx.conf:ro`,
    '--volume', `${resolve(deploy, 'frontend.sh')}:/etc/orch/frontend.sh:ro`,
    '--volume', `${resolve(deploy, 'frontend-csp.sh')}:/etc/orch/frontend-csp.sh:ro`,
    '--volume', `${resolve(process.cwd(), 'dist')}:/usr/share/nginx/html:ro`,
    '--volume', `${directory}:/etc/orch/frontend-tls:ro`,
    '--volume', `${certificate}:/etc/orch/secrets/orch_ca_cert.pem:ro`,
    image, '/bin/sh', '/etc/orch/frontend.sh']);
  const container = stdout.trim();
  if (!/^[a-f0-9]{64}$/.test(container)) throw new Error('The isolated edge container was not identified.');
  try {
    const ca = await readFile(certificate);
    await expect.poll(async () => { try { return (await root(port, ca)).protocol; } catch { return ''; } }).toBe('h2');
    const first = await root(port, ca), next = await root(port, ca);
    const curl = await execute('curl', ['--http2', '--cacert', certificate, '--silent', '--show-error', '--output', '/dev/null',
      '--write-out', '%{http_version}\n', `https://127.0.0.1:${port}/`]);
    expect(curl.stdout.trim()).toBe('2');
    const policy = (reply: typeof first) => String(reply.headers['content-security-policy'] ?? '');
    const nonce = (reply: typeof first) => /'nonce-([a-f0-9]+)'/.exec(policy(reply))?.[1];
    expect(first.headers[':status']).toBe(200); expect(nonce(first)).toBeDefined();
    expect(first.body).toContain(`nonce="${nonce(first)}"`);
    expect(first.body).not.toContain('__ORCH_CSP_NONCE__');
    expect(/script-src ([^;]+)/.exec(policy(first))?.[1]).not.toContain("'unsafe-inline'");
    expect(nonce(next)).not.toBe(nonce(first));
    expect(first.ticket).toBeDefined();
    const resumed = await root(port, ca, first.ticket);
    expect(resumed.reused).toBe(true);
    expect(first.headers['alt-svc']).toBe(`h3=":${port}"; ma=86400`);
    expect((await execute('docker', ['port', container, '443/udp'])).stdout).toContain(`:${port}`);
    // Node verified the synthetic CA above. This context exercises the actual document policy,
    // without installing this test-only authority into the runner's device trust store.
    const context = await browser.newContext({ ignoreHTTPSErrors: true });
    try {
      const page = await context.newPage();
      const login = await context.request.get(`https://127.0.0.1:${port}/login`, {
        headers: { authorization: 'Bearer fixture', cookie: 'session=fixture' },
      });
      expect(login.status()).toBe(200);
      const supplied = await page.evaluate((html) => {
        const template = new DOMParser().parseFromString(html, 'text/html').querySelector<HTMLTemplateElement>('template[data-orch-login-meta]');
        return JSON.parse(template?.content.textContent ?? 'null') as unknown;
      }, await login.text());
      expect(supplied).toEqual(anonymous);
      const cold = await browser.newContext({ ignoreHTTPSErrors: true, locale: 'en-US' });
      try {
        const signIn = await cold.newPage();
        const cdp = await cold.newCDPSession(signIn);
        await cdp.send('Network.enable');
        await cdp.send('Network.emulateNetworkConditions', { offline: false, latency: 200, downloadThroughput: -1, uploadThroughput: -1 });
        const reads: string[] = [];
        signIn.on('request', (request) => { reads.push(new URL(request.url()).pathname); });
        await signIn.goto(`https://127.0.0.1:${port}/login`, { waitUntil: 'commit' });
        const draw = await signIn.waitForFunction(() => {
          const user = document.querySelector<HTMLInputElement>('input[autocomplete=username]');
          const password = document.querySelector<HTMLInputElement>('input[autocomplete=current-password]');
          if (!user?.getBoundingClientRect().height || user.disabled || !password?.getBoundingClientRect().height || password.disabled) return false;
          return performance.now();
        });
        const elapsed = await draw.jsonValue();
        process.stdout.write(`Cold sign-in form at 200 ms: ${String(elapsed)} ms\n`);
        const words = await signIn.locator('link[rel=preload][as=fetch][href^="/assets/words/"]').evaluateAll((links) => links.map((link) => link.getAttribute('href')));
        expect(words.length).toBeGreaterThan(0);
        for (const file of words) {
          expect(file).toMatch(/^\/assets\/words\/en-[a-f0-9]+\.json$/);
          expect(reads.filter((path) => path === file)).toHaveLength(1);
        }
        expect(reads).not.toContain('/api/v1/meta');
        expect(elapsed, await openingTimings(signIn)).toBeLessThanOrEqual(1000);
      } finally { await cold.close(); }
      const fontRefusals: string[] = [];
      page.on('console', (message) => { if (message.type() === 'error' && /font-src|refused.*font/i.test(message.text())) fontRefusals.push(message.text()); });
      await page.goto(`https://127.0.0.1:${port}/`);
      await expect.poll(() => page.evaluate(() => ['orch/start/html-session-asked', 'orch/start/script']
        .map((name) => performance.getEntriesByName(name, 'mark').length))).toEqual([1, 1]);
      const fonts = await page.evaluate(async () => {
        const loaded = await Promise.all([document.fonts.load('400 16px "Inter Variable"'), document.fonts.load('400 16px "JetBrains Mono Variable"')]);
        return { paths: [...document.querySelectorAll<HTMLLinkElement>('link[rel="preload"][as="font"]')].map((link) => new URL(link.href).pathname),
          faces: loaded.map((faces) => faces.length),
          loaded: [document.fonts.check('400 16px "Inter Variable"'), document.fonts.check('400 16px "JetBrains Mono Variable"')] };
      });
      expect(fonts.paths).toHaveLength(2);
      expect(fonts.paths.every((path) => path.startsWith('/fonts/') && path.endsWith('.woff2'))).toBe(true);
      expect(fonts.loaded).toEqual([true, true]);
      expect(fonts.faces.every((count) => count > 0)).toBe(true);
      expect(fontRefusals).toEqual([]);
      expect(/font-src ([^;]+)/.exec(policy(first))?.[1]).toBe("'self'");
      for (const match of first.body.matchAll(/href="([^"\s]+\.css)"/g)) {
        const css = await readFile(resolve(process.cwd(), 'dist', (match[1] ?? '').replace(/^\//, '')), 'utf8');
        const faces = [...css.matchAll(/@font-face\s*\{[^}]*\}/g)].map((face) => face[0]);
        expect(faces.length).toBeGreaterThan(0);
        expect(faces.every((face) => !face.includes('data:') && face.includes('/fonts/'))).toBe(true);
      }
    } finally { await context.close(); }
    // A saved policy is a complete replacement. The same process adopts it without a restart.
    await writeFile(`${policyFile}.next`, transport({ http3: false }));
    await rename(`${policyFile}.next`, policyFile);
    await expect.poll(async () => (await root(port, ca)).headers['alt-svc']).toBe('clear');
    const build = await execute('docker', ['exec', container, 'nginx', '-V']);
    expect(build.stderr).toContain('--with-http_v3_module');
    process.stdout.write(`Stock edge protocol proof ${JSON.stringify({ alpn: first.protocol, resumed: resumed.reused, http3Module: true,
      tlsBuild: /built with ([^\n]+)/.exec(build.stderr)?.[1], udpPublished: true, reload: 'graceful', htmlScripts: 'authorized' })}\n`);
  } finally { await execute('docker', ['rm', '--force', container]); }
});
