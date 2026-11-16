/** The shipped edge's TLS negotiation and inline request policy, without a browser or API credentials. */
import { expect, test } from '@playwright/test';
import { execFile } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdir, readFile, symlink } from 'node:fs/promises';
import { connect, type IncomingHttpHeaders } from 'node:http2';
import { resolve } from 'node:path';
import { promisify } from 'node:util';

const execute = promisify(execFile);

async function root(port: number, ca: Buffer): Promise<{ protocol: string; headers: IncomingHttpHeaders; body: string }> {
  return new Promise((accept, reject) => {
    const session = connect(`https://127.0.0.1:${port}`, { ca, servername: 'orch.test' });
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
      request.once('end', () => { session.close(); accept({ protocol, headers, body }); });
      request.end();
    });
  });
}

test('the stock HTTPS edge negotiates h2 and authorizes each document’s own inline starter', async () => {
  const info = test.info();
  const project = resolve(process.cwd(), '..'), deploy = resolve(project, 'deploy');
  const compose = await readFile(resolve(deploy, 'compose.yml'), 'utf8');
  const image = /^\s+image:\s+(nginx:\S+)\s*$/m.exec(compose)?.[1];
  if (image === undefined) throw new Error('The shipped edge image is required.');
  const directory = info.outputPath('tls'), generation = resolve(directory, 'generation');
  await mkdir(generation, { recursive: true });
  const certificate = resolve(generation, 'chain.pem');
  await execute('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=orch.test',
    '-addext', 'subjectAltName=DNS:orch.test,IP:127.0.0.1', '-keyout', resolve(generation, 'key.pem'), '-out', certificate]);
  await symlink('generation', resolve(directory, 'current'));
  const { stdout } = await execute('docker', ['run', '--detach', '--name', `orch-edge-${randomUUID()}`,
    '--publish', '127.0.0.1::443', '--env', 'ORCH_FRONTEND_HTTPS_BIND=127.0.0.1:443',
    '--volume', `${resolve(deploy, 'nginx.conf')}:/etc/nginx/nginx.conf:ro`,
    '--volume', `${resolve(deploy, 'frontend.sh')}:/etc/orch/frontend.sh:ro`,
    '--volume', `${resolve(process.cwd(), 'dist')}:/usr/share/nginx/html:ro`,
    '--volume', `${directory}:/etc/orch/frontend-tls:ro`,
    '--volume', `${certificate}:/etc/orch/secrets/orch_ca_cert.pem:ro`,
    image, '/bin/sh', '/etc/orch/frontend.sh']);
  const container = stdout.trim();
  if (!/^[a-f0-9]{64}$/.test(container)) throw new Error('The isolated edge container was not identified.');
  try {
    const address = (await execute('docker', ['port', container, '443/tcp'])).stdout.trim();
    const port = Number(/:(\d+)$/.exec(address)?.[1]);
    if (!Number.isInteger(port) || port <= 0) throw new Error('The isolated edge needs its own TLS port.');
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
    const build = await execute('docker', ['exec', container, 'nginx', '-V']);
    process.stdout.write(`Stock edge protocol proof ${JSON.stringify({ alpn: first.protocol, http3Module: build.stderr.includes('--with-http_v3_module'), udpPublished: false })}\n`);
  } finally { await execute('docker', ['rm', '--force', container]); }
});
