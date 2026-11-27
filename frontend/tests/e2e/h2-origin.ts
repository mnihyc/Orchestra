/**
 * The built application over HTTP/2 with TLS, as the shipped edge serves it, with the fake API behind it.
 *
 * The browser cases that time a first opening at a long round trip measure the transport a person gets: the shipped edge
 * negotiates h2 (`first-wave-edge.spec.ts` checks it), so one connection carries every file of the first wave, where the
 * HTTP/1.1 of `vite preview` lets a browser open six connections to a server and sends thirty files in five round trips.
 * Like the edge: `/login` is its own document, a path under `/assets/` or `/fonts/` is that file (kept for a year) or
 * nothing, any other path is the application's document (never kept), and `/api` and `/health` go to the API with their
 * answers passed on as they come (a page's first reads are lines that arrive one by one). A WebSocket is passed through.
 */
import { execFile } from 'node:child_process';
import { X509Certificate, createHash, randomBytes } from 'node:crypto';
import { mkdir, readFile, stat } from 'node:fs/promises';
import { request } from 'node:http';
import { createSecureServer, type Http2ServerRequest, type Http2ServerResponse } from 'node:http2';
import { connect, type Socket } from 'node:net';
import { extname, join, resolve, sep } from 'node:path';
import { promisify } from 'node:util';

const TYPES: Readonly<Record<string, string>> = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8',
  '.json': 'application/json', '.woff2': 'font/woff2', '.svg': 'image/svg+xml', '.png': 'image/png', '.ico': 'image/x-icon',
  '.webmanifest': 'application/manifest+json', '.txt': 'text/plain; charset=utf-8', '.wasm': 'application/wasm',
};
/** What names the hop and not the message: a proxy does not pass it on. */
const HOP = new Set(['connection', 'keep-alive', 'transfer-encoding', 'upgrade', 'proxy-connection', 'te', 'trailer', 'http2-settings']);
const FILES = /^\/(?:assets|fonts)\//;
const API = /^\/(?:api|health)(?:\/|$)/;

/** `key` is the SHA-256 of its certificate's public key (base64), for a browser that is to trust it
 * (`--ignore-certificate-errors-spki-list`). */
export interface H2Origin { readonly origin: string; readonly key: string; close: () => Promise<void> }

function passed(headers: Readonly<Record<string, string | string[] | undefined>>): Record<string, string | string[]> {
  const out: Record<string, string | string[]> = {};
  for (const [name, value] of Object.entries(headers)) if (!name.startsWith(':') && !HOP.has(name) && value !== undefined) out[name] = value;
  return out;
}

function proxy(api: URL, incoming: Http2ServerRequest, outgoing: Http2ServerResponse): void {
  const upstream = request({ host: api.hostname, port: api.port, method: incoming.method, path: incoming.url,
    headers: { ...passed(incoming.headers), host: api.host } }, (answer) => {
    outgoing.writeHead(answer.statusCode ?? 502, passed(answer.headers));
    answer.pipe(outgoing);
  });
  upstream.on('error', () => { if (!outgoing.headersSent) outgoing.writeHead(502); outgoing.end(); });
  // A read the page lets go ends its request to the API too.
  outgoing.on('close', () => { upstream.destroy(); });
  incoming.pipe(upstream);
}

async function file(dist: string, incoming: Http2ServerRequest, outgoing: Http2ServerResponse): Promise<void> {
  const path = decodeURIComponent(new URL(incoming.url, 'https://origin.invalid').pathname);
  let target = /^\/login\/?$/.test(path) ? join(dist, 'login.html') : resolve(dist, `.${path}`);
  if (target !== dist && !target.startsWith(dist + sep)) { outgoing.writeHead(404); outgoing.end(); return; }
  if (!(await stat(target).then((found) => found.isFile(), () => false))) {
    if (FILES.test(path)) { outgoing.writeHead(404); outgoing.end(); return; }
    target = join(dist, 'index.html');
  }
  let body = await readFile(target);
  // The edge gives each document its own nonce for its inline starter.
  if (extname(target) === '.html') body = Buffer.from(body.toString('utf8').replaceAll('__ORCH_CSP_NONCE__', randomBytes(16).toString('hex')));
  outgoing.writeHead(200, { 'content-type': TYPES[extname(target)] ?? 'application/octet-stream',
    'cache-control': FILES.test(path) ? 'public, max-age=31536000, immutable' : 'no-store' });
  outgoing.end(body);
}

/** Serves `dist` over h2 on a free port of this machine, with `api` behind it and its certificate in `directory` (a test's
 * own output, which the run clears); `close` stops it. */
export async function h2Origin(dist: string, api: URL, directory: string): Promise<H2Origin> {
  await mkdir(directory, { recursive: true });
  const keyFile = join(directory, 'key.pem'), certificateFile = join(directory, 'certificate.pem');
  await promisify(execFile)('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=127.0.0.1',
    '-addext', 'subjectAltName=IP:127.0.0.1', '-keyout', keyFile, '-out', certificateFile]);
  const root = resolve(dist);
  const server = createSecureServer({ key: await readFile(keyFile), cert: await readFile(certificateFile), allowHTTP1: true },
    (incoming, outgoing) => {
      if (API.test(new URL(incoming.url, 'https://origin.invalid').pathname)) { proxy(api, incoming, outgoing); return; }
      file(root, incoming, outgoing).catch(() => { if (!outgoing.headersSent) outgoing.writeHead(500); outgoing.end(); });
    });
  // The browser opens a WebSocket on a connection of its own (HTTP/1.1): its bytes go to the API as they are.
  const sockets = new Set<Socket>();
  server.on('upgrade', (incoming: { method?: string; url?: string; rawHeaders: string[] }, socket: Socket, head: Buffer) => {
    const upstream = connect(Number(api.port), api.hostname, () => {
      const lines = [`${incoming.method ?? 'GET'} ${incoming.url ?? '/'} HTTP/1.1`];
      for (let at = 0; at + 1 < incoming.rawHeaders.length; at += 2) lines.push(`${incoming.rawHeaders[at] ?? ''}: ${incoming.rawHeaders[at + 1] ?? ''}`);
      upstream.write(`${lines.join('\r\n')}\r\n\r\n`);
      upstream.write(head);
      socket.pipe(upstream).pipe(socket);
    });
    for (const each of [socket, upstream]) { sockets.add(each); each.on('close', () => sockets.delete(each)); }
    upstream.on('error', () => { socket.destroy(); });
    socket.on('error', () => { upstream.destroy(); });
  });
  server.on('connection', (socket: Socket) => { sockets.add(socket); socket.on('close', () => sockets.delete(socket)); });
  await new Promise<void>((done) => { server.listen(0, '127.0.0.1', done); });
  const address = server.address();
  if (address === null || typeof address === 'string') throw new Error('The h2 origin needs a TCP port.');
  const certificate = new X509Certificate(await readFile(certificateFile));
  return {
    origin: `https://127.0.0.1:${address.port}`,
    key: createHash('sha256').update(certificate.publicKey.export({ type: 'spki', format: 'der' })).digest('base64'),
    close: async () => {
      for (const socket of sockets) socket.destroy();
      await new Promise<void>((done) => { server.close(() => { done(); }); });
    },
  };
}
