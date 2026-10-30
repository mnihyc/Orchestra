/** The browser's socket and picture: a picture's header read as the protocol writes it, a point of the panel taken to the page's own coordinates at any size, one socket that watches carrying nothing, a lost line taken up again, and why a browser ended. */
import { describe, expect, it, vi } from 'vitest';

import { orderBrowsers } from '@/features/browser/Browsers';
import { fit, toPage } from '@/features/browser/Picture';
import { type BrowserSocketLike, connectBrowser, modifiers, readPicture } from '@/features/browser/socket';
import type { Browser } from '@/generated/api-types';

function pictureFrame(n: number, viewport: [number, number], image: [number, number], jpeg: number[]): ArrayBuffer {
  const data = new ArrayBuffer(12 + jpeg.length);
  const view = new DataView(data);
  view.setUint32(0, n);
  view.setUint16(4, viewport[0]);
  view.setUint16(6, viewport[1]);
  view.setUint16(8, image[0]);
  view.setUint16(10, image[1]);
  new Uint8Array(data, 12).set(jpeg);
  return data;
}

class FakeSocket implements BrowserSocketLike {
  readyState = 0;
  binaryType = 'blob';
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: { code: number; reason?: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(readonly url: string) {}
  send(data: string) { this.sent.push(data); }
  close() { this.readyState = 3; }
  open() { this.readyState = 1; this.onopen?.(); }
  text(frame: object) { this.onmessage?.({ data: JSON.stringify(frame) }); }
}

describe('a picture is the tab as the browser draws it', () => {
  it('reads the number, the page size and the image size big-endian before the JPEG', () => {
    const picture = readPicture(pictureFrame(70_000, [1280, 800], [640, 400], [0xff, 0xd8, 0xff, 0xd9]));
    expect(picture?.n).toBe(70_000);
    expect(picture?.viewport).toEqual({ width: 1280, height: 800 });
    expect(picture?.image).toEqual({ width: 640, height: 400 });
    expect([...(picture?.jpeg ?? [])]).toEqual([0xff, 0xd8, 0xff, 0xd9]);
    expect(readPicture(new ArrayBuffer(12))).toBeNull();
  });
  it('fits the page into the panel with its proportions kept', () => {
    expect(fit({ width: 600, height: 600 }, { width: 1280, height: 800 })).toEqual({ width: 600, height: 375 });
    expect(fit({ width: 1000, height: 300 }, { width: 1280, height: 800 })).toEqual({ width: 480, height: 300 });
    expect(fit({ width: 0, height: 300 }, { width: 1280, height: 800 })).toEqual({ width: 0, height: 0 });
  });
  it('takes a point of the picture to the same point of the page, whatever size the picture is drawn at', () => {
    const page = { width: 1280, height: 800 };
    expect(toPage({ x: 160, y: 110 }, { left: 10, top: 10, width: 300, height: 200 }, page)).toEqual({ x: 640, y: 400 });
    expect(toPage({ x: 610, y: 410 }, { left: 10, top: 10, width: 1200, height: 800 }, page)).toEqual({ x: 640, y: 400 });
    expect(toPage({ x: -50, y: 5000 }, { left: 0, top: 0, width: 640, height: 400 }, page)).toEqual({ x: 0, y: 799 });
  });
  it('says the modifiers as the browser protocol counts them', () => {
    expect(modifiers({ altKey: true, ctrlKey: true, metaKey: false, shiftKey: true })).toBe(1 | 2 | 8);
  });
});

describe('one socket at a time, watching or driving', () => {
  it('asks a ticket of the kind wanted, says what it watches, acknowledges what was drawn, and follows the tabs', async () => {
    const sockets: FakeSocket[] = [];
    const tickets: boolean[] = [];
    const pictures: number[] = [];
    const connection = connectBrowser({
      browserId: 'b1',
      control: false,
      onPicture: (picture) => pictures.push(picture.n),
      ticket: (_id, control) => { tickets.push(control); return Promise.resolve('t1'); },
      address: (id, ticket) => `ws://test/${id}?ticket=${ticket}`,
      connect: (url) => { const socket = new FakeSocket(url); sockets.push(socket); return socket; },
    });
    await vi.waitFor(() => expect(sockets).toHaveLength(1));
    const socket = sockets[0]!;
    socket.open();
    connection.watch('tab-a', 600, 375);
    socket.text({ op: 'ready', control: false, tabs: [{ id: 'tab-a', title: 'Roadmap', url: 'https://example.org', loading: false, active: true, opener: null }], viewport: { width: 1280, height: 800 }, people: { watching: [], driving: [] } });
    expect(tickets).toEqual([false]);
    expect(socket.sent.map((frame) => JSON.parse(frame) as { op: string }).filter((frame) => frame.op === 'watch')).not.toHaveLength(0);
    socket.onmessage?.({ data: pictureFrame(7, [1280, 800], [640, 400], [1, 2, 3]) });
    expect(pictures).toEqual([7]);
    connection.drawn(7);
    expect(socket.sent.at(-1)).toBe(JSON.stringify({ op: 'ack', n: 7 }));
    socket.text({ op: 'tabs', tabs: [{ id: 'tab-b', title: 'Sign in', url: 'https://accounts.example.org', loading: false, active: true, opener: 'tab-a' }] });
    expect(connection.state.getState().tab).toBe('tab-b');
    connection.close();
  });

  it('takes a lost line up again with a new ticket, and ends for good on the reason the server gives', async () => {
    vi.useFakeTimers();
    try {
      const sockets: FakeSocket[] = [];
      const connection = connectBrowser({ browserId: 'b1', control: true, onPicture: () => undefined, ticket: () => Promise.resolve('t'), connect: (url) => { const socket = new FakeSocket(url); sockets.push(socket); return socket; } });
      await vi.waitFor(() => expect(sockets).toHaveLength(1));
      sockets[0]!.open();
      sockets[0]!.onclose?.({ code: 1006 });
      expect(connection.state.getState().phase).toBe('lost');
      await vi.advanceTimersByTimeAsync(600);
      expect(sockets).toHaveLength(2);
      sockets[1]!.open();
      sockets[1]!.text({ op: 'closed', reason: 'closed_by_agent' });
      expect(connection.state.getState()).toMatchObject({ phase: 'closed', reason: 'closed_by_agent' });
      connection.close();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('the list puts the browsers that are open first, the thread agent\'s before the rest', () => {
  it('orders by the thread whose agent opened it, and leaves out those closed', () => {
    const row = (id: string, change: Partial<Browser>): Browser => ({ id, environment: { id: 'e' } as Browser['environment'], profile: id, origin: 'started', opened_by: null, thread: null, preset: 'headless', display: null, endpoint: null, program: null, tabs: 1, note: null, watching: 0, driving: [], state: 'open', close_reason: null, opened_at: '', last_used_at: '', closed_at: null, ...change });
    const thread = { id: 't1', kind: 'thread', name: 'Fix login', redacted: false } as Browser['thread'];
    const ordered = orderBrowsers([row('notion', { opened_by: { id: 'u' } as Browser['opened_by'] }), row('gone', { state: 'closed' }), row('agent', { thread })], 't1');
    expect(ordered.map((browser) => browser.id)).toEqual(['agent', 'notion']);
  });
});
