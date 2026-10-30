/** Push in this browser: whether it can, asking its leave, subscribing, and telling the server which browser takes it.
 *
 * A browser takes push through a service worker, which Orch uses for that and nothing else. The person
 * is asked by the browser, never by this page behind its back, and a subscription is kept only while
 * the server knows of it: what was subscribed here and what the server holds are made one by the two
 * acts below.
 */
import { addPushSubscription, removePushSubscription } from '@/api/notifications';

/** Where the service worker is served from, at the root so that it covers every address of the application. */
export const WORKER_PATH = '/sw.js';

/** Why this browser cannot take push, or `null` when it can be asked. */
export type PushBlock = 'unsupported' | 'blocked';

export function pushBlock(): PushBlock | null {
  if (typeof navigator === 'undefined' || !('serviceWorker' in navigator) || typeof window === 'undefined' || !('PushManager' in window) || !('Notification' in window)) {
    return 'unsupported';
  }
  return Notification.permission === 'denied' ? 'blocked' : null;
}

/** The key by which a browser subscribes, as the bytes the browser wants. A key is base64url text. */
export function keyBytes(key: string): Uint8Array<ArrayBuffer> {
  const padded = key.replace(/-/g, '+').replace(/_/g, '/').padEnd(Math.ceil(key.length / 4) * 4, '=');
  const raw = atob(padded);
  const bytes = new Uint8Array(new ArrayBuffer(raw.length));
  for (let at = 0; at < raw.length; at += 1) bytes[at] = raw.charCodeAt(at);
  return bytes;
}

/** The text of a key of a subscription, as base64url without padding: what the server stores. */
export function keyText(buffer: ArrayBuffer | null): string {
  if (buffer === null) return '';
  let raw = '';
  for (const byte of new Uint8Array(buffer)) raw += String.fromCharCode(byte);
  return btoa(raw).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/** The subscription this browser holds now, or `null`. */
export async function currentSubscription(): Promise<PushSubscription | null> {
  if (pushBlock() === 'unsupported') return null;
  const registration = await navigator.serviceWorker.getRegistration(WORKER_PATH);
  return registration === undefined ? null : registration.pushManager.getSubscription();
}

/** The answer of the person to the browser's question: the leave was given, or it was not. */
export class PushRefused extends Error {
  override readonly name = 'PushRefused';
}

/**
 * Asks the browser's leave, subscribes with the system's key and tells the server. Called from a press of
 * the person's, since browsers refuse to ask for anything else.
 */
export async function enablePush(publicKey: string): Promise<PushSubscription> {
  if (pushBlock() === 'unsupported') throw new PushRefused('unsupported');
  const permission = await Notification.requestPermission();
  if (permission !== 'granted') throw new PushRefused(permission);
  const registration = await navigator.serviceWorker.register(WORKER_PATH);
  await navigator.serviceWorker.ready;
  const subscription =
    (await registration.pushManager.getSubscription()) ?? (await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: keyBytes(publicKey) }));
  await addPushSubscription({
    endpoint: subscription.endpoint,
    keys: { p256dh: keyText(subscription.getKey('p256dh')), auth: keyText(subscription.getKey('auth')) },
  });
  return subscription;
}

/** Stops this browser taking push: the server forgets it first, then the browser. */
export async function disablePush(): Promise<void> {
  const subscription = await currentSubscription();
  if (subscription === null) return;
  await removePushSubscription(subscription.endpoint);
  await subscription.unsubscribe();
}
