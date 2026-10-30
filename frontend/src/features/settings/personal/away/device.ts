/** Read this browser's real push features and match only its endpoint hash to an owned server registration. */
import { currentSubscription } from '@/features/settings/personal/push';
import type { PushDevice } from './types';

export interface DeviceFacts { secure: boolean; worker: boolean; push: boolean; notifications: boolean; permission: NotificationPermission | null }
export type DeviceState = 'insecure' | 'unavailable' | 'blocked' | 'default' | 'missing' | 'registered';

export function browserFacts(): DeviceFacts {
  return {
    secure: window.isSecureContext,
    worker: 'serviceWorker' in navigator,
    push: 'PushManager' in window,
    notifications: 'Notification' in window,
    permission: 'Notification' in window ? Notification.permission : null,
  };
}

/** This read does not register a worker, subscribe a device or ask for permission. */
export async function inspectDevice(): Promise<{ facts: DeviceFacts; hash: string | null }> {
  const facts = browserFacts();
  if (!facts.secure || !facts.worker || !facts.push || !facts.notifications) return { facts, hash: null };
  const subscription = await currentSubscription();
  if (subscription === null) return { facts, hash: null };
  const bytes = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(subscription.endpoint));
  return { facts, hash: [...new Uint8Array(bytes)].map((byte) => byte.toString(16).padStart(2, '0')).join('') };
}

export function deviceState(facts: DeviceFacts, hash: string | null, devices: readonly PushDevice[]): DeviceState {
  if (!facts.secure) return 'insecure';
  if (!facts.worker || !facts.push || !facts.notifications) return 'unavailable';
  if (facts.permission === 'denied') return 'blocked';
  if (facts.permission !== 'granted') return 'default';
  return hash !== null && devices.some((device) => device.endpoint_hash === hash) ? 'registered' : 'missing';
}
