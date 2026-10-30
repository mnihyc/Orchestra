/** A successful setup on this device; native binding options revalidate it before reuse. */
import { useViewChoice, writeViewChoice } from '@/lib/viewChoices';
import type { Picks } from './choices';

export interface LastSetup { place: string; picks: Picks }

export function readLastSetup(raw: string): LastSetup | null {
  try {
    const value: unknown = JSON.parse(raw);
    if (typeof value !== 'object' || value === null || !('place' in value) || typeof value.place !== 'string' || !('picks' in value)) return null;
    const picks = value.picks;
    if (typeof picks !== 'object' || picks === null) return null;
    const text = (key: string): string | null => {
      const field: unknown = Reflect.get(picks, key);
      return typeof field === 'string' ? field : null;
    };
    const arrangement = text('arrangement');
    if (arrangement !== null && arrangement !== 'separated' && arrangement !== 'key_separated' && arrangement !== 'credential_shared' && arrangement !== 'handed_over' && arrangement !== 'together') return null;
    return { place: value.place, picks: { provider: text('provider'), agent: text('agent'), model: text('model'), effort: text('effort'), environment: text('environment'), arrangement } };
  } catch { return null; }
}

export function useLastSetup(): LastSetup | null {
  const [raw] = useViewChoice<string>('thread', 'setup', '');
  return readLastSetup(raw);
}

export function rememberSetup(user: string | null, setup: LastSetup): void {
  writeViewChoice(user, 'thread', 'setup', JSON.stringify(setup));
}
