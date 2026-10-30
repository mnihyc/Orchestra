/** Counts what the page itself asks for. Sign-in, preference setup and sheet images are outside this meter. */
import type { Page, Request, Response, WebSocket } from '@playwright/test';
import { IDLE_MS, QUIET_MS, READY_MS } from './config.ts';

export type Phase = 'first_load' | 'opening' | 'idle' | 'after_idle';
export interface RequestRow { phase: Phase; method: string; endpoint: string; at_ms: number; status: number | null; failed: boolean; finished: boolean }
export interface PhaseRequests { count: number; requests: RequestRow[] }
export interface RequestReceipt {
  first_load: PhaseRequests;
  opening: PhaseRequests;
  idle: PhaseRequests & { observed_ms: number; required_ms: number };
  after_idle: PhaseRequests;
  live_connections: { endpoint: string; phase: Phase; at_ms: number }[];
  budgets: { first_load_max: number; idle_max: number; first_load_ok: boolean; idle_ok: boolean; idle_complete: boolean };
}

/** Keep method/route/query names, never tickets, password-link tokens or query values. */
export function safeEndpoint(raw: string): string {
  const url = new URL(raw, 'http://capture.invalid');
  let path = url.pathname.replace(/\/(reset|invite|verify-email|handover|callback)\/[^/]+/g, '/$1/:redacted');
  path = path.split('/').map((part) => /^[\da-f]{8}-(?:[\da-f]{4}-){3}[\da-f]{12}$/i.test(part) ? ':id' : part.length >= 32 ? ':opaque' : part).join('/');
  const keys: string[] = [];
  url.searchParams.forEach((_value, key) => keys.push(encodeURIComponent(key)));
  return path + (keys.length === 0 ? '' : `?${[...new Set(keys)].sort().map((key) => `${key}=redacted`).join('&')}`);
}

export function requestReceipt(rows: readonly RequestRow[], live: RequestReceipt['live_connections'], observedMs: number): RequestReceipt {
  const phase = (name: Phase): PhaseRequests => { const requests = rows.filter((row) => row.phase === name); return { count: requests.length, requests }; };
  const first = phase('first_load'); const idle = phase('idle');
  return { first_load: first, opening: phase('opening'), idle: { ...idle, observed_ms: observedMs, required_ms: IDLE_MS }, after_idle: phase('after_idle'), live_connections: live,
    budgets: { first_load_max: 8, idle_max: 0, first_load_ok: first.count <= 8, idle_ok: idle.count === 0, idle_complete: observedMs >= IDLE_MS } };
}

export class PageRequests {
  private readonly started = performance.now();
  private lastActivity = this.started;
  private phase: Phase = 'first_load';
  private readonly rows = new Map<Request, RequestRow>();
  private readonly live: RequestReceipt['live_connections'] = [];
  private readonly inFlight = new Set<Request>();
  private readonly page: Page;
  private readonly origin: string;

  constructor(page: Page, origin: string) {
    this.page = page; this.origin = origin;
    page.on('request', this.request);
    page.on('requestfinished', this.finished);
    page.on('requestfailed', this.failed);
    page.on('response', this.response);
    page.on('websocket', this.socket);
  }

  private request = (request: Request): void => {
    const url = new URL(request.url());
    if (request.resourceType() === 'websocket' && url.pathname === '/api/v1/ws') return;
    // Every finite network request affects readiness, including assets and cross-origin images.
    if (!['data:', 'blob:'].includes(url.protocol)) { this.inFlight.add(request); this.lastActivity = performance.now(); }
    if (url.origin !== this.origin || !url.pathname.startsWith('/api/')) return;
    this.rows.set(request, { phase: this.phase, method: request.method(), endpoint: safeEndpoint(request.url()), at_ms: performance.now() - this.started,
      status: null, failed: false, finished: false });
  };
  private response = (response: Response): void => {
    const request = response.request();
    const row = this.rows.get(request);
    if (row !== undefined) row.status = response.status();
    // Only the application's actual live transport is exempt. An arbitrary hanging HTTP request fails readiness.
    if (new URL(request.url()).pathname === '/api/v1/ws' && response.status() === 101) {
      this.inFlight.delete(request); this.rows.delete(request);
    }
  };
  private finished = (request: Request): void => {
    this.inFlight.delete(request); this.lastActivity = performance.now();
    const row = this.rows.get(request); if (row !== undefined) row.finished = true;
  };
  private failed = (request: Request): void => {
    this.finished(request);
    const row = this.rows.get(request); if (row !== undefined) row.failed = true;
  };
  private socket = (socket: WebSocket): void => {
    this.live.push({ endpoint: safeEndpoint(socket.url()), phase: this.phase, at_ms: performance.now() - this.started });
  };

  begin(phase: Phase): void { this.phase = phase; }
  async quiet(): Promise<void> {
    const deadline = performance.now() + READY_MS;
    while (this.inFlight.size !== 0 || performance.now() - this.lastActivity < QUIET_MS) {
      if (this.page.isClosed() || performance.now() >= deadline) throw new Error('The page did not reach network idle within 30 seconds.');
      await new Promise<void>((done) => setTimeout(done, 50));
    }
  }
  receipt(observedMs: number): RequestReceipt { return requestReceipt([...this.rows.values()], [...this.live], observedMs); }
  stop(): void {
    this.page.off('request', this.request); this.page.off('requestfinished', this.finished); this.page.off('requestfailed', this.failed);
    this.page.off('response', this.response); this.page.off('websocket', this.socket);
  }
}
