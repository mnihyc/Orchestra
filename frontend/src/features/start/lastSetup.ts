/** A successful setup kept by the server; native binding options revalidate it before reuse. */
import { useQuery } from '@tanstack/react-query';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { meStateQuery } from '@/api/personal';
import { requestReader } from '@/lib/readAttempts';
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

/** Read facts belong to the same private server value used by the start controls. */
export interface LastSetupObservation {
  reader: string;
  saved: Picks | undefined;
  status: 'pending' | 'success' | 'error';
  unavailable: boolean;
}

export function useLastSetup(): LastSetup | null {
  return useLastSetupObservation()[0];
}

/** Opening the facts observes this same read and never reads browser storage. */
export function useLastSetupObservation(): [LastSetup | null, LastSetupObservation?] {
  const person = useStore(client.session, (state) => state.status === 'signed_in' ? state.userId : null);
  const reader = useStore(client.session, (state) => requestReader(state.userId, state.readerVersion));
  const state = useQuery({ ...meStateQuery, enabled: person !== null, staleTime: Infinity });
  const saved = person === null ? undefined : state.data?.thread_setup;
  const value = saved === undefined ? null : readLastSetup(JSON.stringify(saved));
  return [value, person === null ? undefined : { reader, saved: value?.picks, status: state.status,
    unavailable: saved !== undefined && value === null }];
}
