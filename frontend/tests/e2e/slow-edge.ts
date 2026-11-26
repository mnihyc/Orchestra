/** A hosted-only HTTP/1.1 edge holds real connections for one simulated long round trip. */
import { Agent, createServer, request } from 'node:http';
import { connect, type Socket } from 'node:net';
import { createGzip } from 'node:zlib';

export async function slowEdge(upstream: string, roundTripMs: number) {
  const target = new URL(upstream);
  if (target.protocol !== 'http:') throw new Error('The isolated latency edge forwards the HTTP test server.');
  const agent = new Agent({ keepAlive: true });
  const sockets = new Set<Socket>(), timers = new Set<ReturnType<typeof setTimeout>>();
  const reads: { path: string; at: number }[] = [];
  let htmlAt: number | undefined;
  const server = createServer((incoming, response) => {
    const path = new URL(incoming.url ?? '/', target).pathname;
    reads.push({ path, at: performance.now() });
    const outgoing = request(new URL(incoming.url ?? '/', target), { agent, method: incoming.method, headers: incoming.headers }, (answer) => {
      const timer = setTimeout(() => {
        timers.delete(timer);
        if (response.destroyed) { answer.destroy(); return; }
        if (answer.headers['content-type']?.includes('text/html') === true) htmlAt = performance.now();
        // Match the shipped edge's conservative gzip level while retaining real held connections.
        const compressed = /\.(js|css)$/.test(path) && answer.headers['content-encoding'] === undefined;
        const headers = { ...answer.headers };
        if (compressed) { delete headers['content-length']; headers['content-encoding'] = 'gzip'; headers.vary = 'Accept-Encoding'; }
        response.writeHead(answer.statusCode ?? 502, headers);
        if (compressed) answer.pipe(createGzip({ level: 1 }).on('error', () => response.destroy())).pipe(response);
        else answer.pipe(response);
      }, roundTripMs);
      timers.add(timer);
    });
    outgoing.on('error', () => { if (!response.headersSent) response.writeHead(502); response.end(); });
    response.on('close', () => { outgoing.destroy(); });
    incoming.pipe(outgoing);
  });
  server.on('connection', (socket) => { sockets.add(socket); socket.on('close', () => { sockets.delete(socket); }); });
  server.on('upgrade', (incoming, socket, head) => {
    const remote = connect({ host: target.hostname, port: Number(target.port) || 80 });
    remote.once('connect', () => {
      const headers: string[] = [];
      for (let index = 0; index < incoming.rawHeaders.length; index += 2) headers.push(`${incoming.rawHeaders[index] ?? ''}: ${incoming.rawHeaders[index + 1] ?? ''}`);
      remote.write(`${incoming.method ?? 'GET'} ${incoming.url ?? '/'} HTTP/${incoming.httpVersion}\r\n${headers.join('\r\n')}\r\n\r\n`);
      if (head.length > 0) remote.write(head);
      remote.pipe(socket); socket.pipe(remote);
    });
    remote.on('error', () => { socket.destroy(); });
    socket.on('error', () => { remote.destroy(); });
    socket.on('close', () => { remote.destroy(); });
  });
  await new Promise<void>((accept, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', accept); });
  const address = server.address();
  if (address === null || typeof address === 'string') throw new Error('The latency edge needs its own TCP port.');
  return {
    origin: `http://127.0.0.1:${address.port}`,
    waves(files: readonly string[]) {
      if (htmlAt === undefined) throw new Error('The latency edge must have served the document.');
      const documentAt = htmlAt;
      return reads.filter((read) => files.includes(read.path.slice(1))).map((read) => ({ file: read.path, wave: Math.floor((read.at - documentAt) / roundTripMs) + 1 }));
    },
    async close() {
      for (const timer of timers) clearTimeout(timer);
      for (const socket of sockets) socket.destroy();
      agent.destroy();
      await new Promise<void>((accept, reject) => { server.close((error) => { if (error !== undefined) reject(error); else accept(); }); });
    },
  };
}
