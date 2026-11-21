/** Inspect just the greeting of one explicitly admitted personal TCP use. */
import { api, ok } from '@/api/client';
import { serverClock } from '@/lib/serverClock';

const supported = ['RFB 003.003\n', 'RFB 003.007\n', 'RFB 003.008\n'];

/** A WebSocket message can contain part of a greeting or more than the whole greeting. */
export function displayGreeting() {
  let prefix = '';
  return (chunk: Uint8Array): boolean | undefined => {
    for (const byte of chunk) {
      prefix += String.fromCharCode(byte);
      if (supported.includes(prefix)) return true;
      if (!supported.some((version) => version.startsWith(prefix))) return false;
    }
    return undefined;
  };
}

export async function checkPortDisplay(environment: string, port: number, signal: AbortSignal): Promise<boolean | null> {
  const use = await ok(api.POST('/environments/{id}/ports/{port}/use', {
    params: { path: { id: environment, port: String(port) } },
    body: { transport: 'tcp', opening_id: null }, signal,
  }));
  signal.throwIfAborted();
  if (!('ticket' in use)) return null;
  const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const socket = new WebSocket(`${scheme}//${location.host}/api/v1/ws/ports/${encodeURIComponent(use.id)}?ticket=${encodeURIComponent(use.ticket)}`, 'orch.port.v1');
  socket.binaryType = 'arraybuffer';
  return new Promise((resolve) => {
    const greeting = displayGreeting();
    const finish = (rfb: boolean | null) => {
      clearTimeout(timer);
      signal.removeEventListener('abort', cancelled);
      socket.onmessage = null;
      socket.onclose = null;
      socket.onerror = null;
      socket.close();
      resolve(rfb);
    };
    const cancelled = () => finish(null);
    const remaining = Date.parse(use.ticket_expires_at) - (Date.now() + (serverClock.offset() ?? 0));
    const timer = setTimeout(cancelled, Math.max(0, remaining));
    signal.addEventListener('abort', cancelled, { once: true });
    socket.onclose = cancelled;
    socket.onerror = cancelled;
    socket.onmessage = ({ data }: MessageEvent<unknown>) => {
      const result = data instanceof ArrayBuffer ? greeting(new Uint8Array(data)) : false;
      if (result !== undefined) finish(result);
    };
  });
}
