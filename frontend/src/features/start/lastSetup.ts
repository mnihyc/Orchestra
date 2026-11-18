/** A successful setup on this device; native binding options revalidate it before reuse. */
import { useViewChoice, writeViewChoice } from '@/lib/viewChoices';
import { NO_PICKS, type Picks } from './choices';
import type { DeviceChoiceObservation } from '@/lib/viewChoiceFacts';

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
  return useLastSetupObservation()[0];
}

/** Metadata shares the existing successful-setup read; no read is added when its facts open. */
export function useLastSetupObservation(): [LastSetup | null, DeviceChoiceObservation<Picks>?] {
  const [raw, , observation] = useViewChoice<string>('thread', 'setup', '');
  const value = readLastSetup(raw);
  const saved = typeof observation?.saved === 'string' ? readLastSetup(observation.saved) : null;
  return [value, observation === undefined ? undefined : { ...observation, chosen: value?.picks, effective: value?.picks ?? NO_PICKS, saved: saved?.picks,
    saved_choice_availability: raw !== '' && value === null ? 'unavailable' : observation.saved_choice_availability }];
}

export function rememberSetup(user: string | null, setup: LastSetup): void {
  writeViewChoice(user, 'thread', 'setup', JSON.stringify(setup));
}
