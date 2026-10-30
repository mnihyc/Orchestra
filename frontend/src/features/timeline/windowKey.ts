/** A transport refresh retires requests without resetting this history's scroll and link intent. */
import { useState } from 'react';
import type { ThreadView } from '@/realtime/stores/thread';

export function useTimelineWindowKey(view: ThreadView): string {
  const lifetime = view.read?.lifetime ?? 0;
  const projection = view.read?.projection;
  const known = projection === undefined || projection === null ? null : JSON.stringify([
    projection.viewed_continuation_id, projection.projection_revision, projection.follows_active,
    projection.follows_active ? projection.execution_revision : null,
  ]);
  const [held, setHeld] = useState({ lifetime, known });
  if (held.lifetime !== lifetime || known !== null && held.known !== known) setHeld({ lifetime, known });
  return JSON.stringify([lifetime, view.read?.selection ?? null, known ?? (held.lifetime === lifetime ? held.known : null)]);
}
