/** The place a person was last in, kept with their interface state so that opening the application on any device comes back to it (`sidebar.last_place`).
 *
 * The device keeps the place immediately. The authorized server read carries the portable visit
 * without an extra HTTP write. Opening `/` reads it back and goes
 * there when the place can still be read; a place that cannot is ignored, and nothing of it is shown.
 */
import { useLocation } from '@tanstack/react-router';
import { useEffect } from 'react';

import { client } from '@/api/client';
import { internalAction, record } from '@/features/onboarding/state';
import { useCurrentHeading } from '@/shell/screen';

export interface LastPlace {
  node_id: string;
  route: string;
}

/** The place a person's interface state says they were last in, where it says one. */
export function savedPlace(state: Record<string, unknown> | undefined): LastPlace | null {
  const place = record(record(state?.sidebar).last_place);
  if (typeof place.node_id !== 'string' || typeof place.route !== 'string') return null;
  const route = internalAction(place.route);
  return route === null ? null : { node_id: place.node_id, route };
}

const deviceKey = (user: string) => `orch.${user}.last-place`;

export function devicePlace(): LastPlace | null {
  const user = client.session.getState().userId;
  if (user === null) return null;
  try { return savedPlace({ sidebar: { last_place: JSON.parse(localStorage.getItem(deviceKey(user)) ?? 'null') as unknown } }); }
  catch { return null; }
}

/** A cached page changes this device only; its next server read carries the portable location. */
export function rememberPlace(place: LastPlace): void {
  const user = client.session.getState().userId;
  if (user === null || internalAction(place.route) === null) return;
  try { localStorage.setItem(deviceKey(user), JSON.stringify(place)); }
  catch { /* The served portable place remains available if device storage is closed. */ }
}

/** Follows the screen in view and remembers the place it shows. Called once, by the frame of the application. */
export function useRememberPlace(): void {
  const nodeId = useCurrentHeading()?.nodeId;
  const route = useLocation({ select: (location) => `${location.pathname}${location.searchStr}` });
  useEffect(() => {
    if (nodeId === undefined) return;
    const safe = internalAction(route);
    if (safe !== null) rememberPlace({ node_id: nodeId, route: safe });
  }, [nodeId, route]);
}
