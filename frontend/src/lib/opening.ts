/** A logical opening survives its HTTP, cached-view and live reads, including reconnects. */
import { newActKey } from '@/lib/requests';

let current: string | undefined;

export function openingId(): string {
  current ??= newActKey();
  return current;
}

/** Called before route loading, so its first request and subscription share the same nonce. */
export function beginOpening(): void { current = newActKey(); }
