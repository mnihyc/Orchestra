import { useEffect } from 'react';
import { createStore } from 'zustand/vanilla';
import { client } from '@/api/client';
import { requestReader } from '@/lib/readAttempts';
import type { DeviceChoiceObservation } from '@/lib/viewChoiceFacts';

type PickerGroupReport = Pick<DeviceChoiceObservation, 'reader' | 'origin' | 'saved_choice_availability' | 'read_result' | 'write_result'> & {
  current: number | undefined;
  saved: number | undefined;
};

/** Only the last open picker's counts and storage outcome survive closing its popup. */
export const pickerGroupReport = createStore<PickerGroupReport | null>(() => null);

client.session.subscribe((next, previous) => {
  if (next.readerVersion !== previous.readerVersion || next.userId !== previous.userId) pickerGroupReport.setState(null, true);
});

export function useReportPickerGroups(open: boolean, receipt: DeviceChoiceObservation | undefined, searching: boolean, current: number | undefined, hasGroups: boolean) {
  const reader = receipt?.reader;
  const origin = searching ? 'page' : receipt?.origin;
  const availability = receipt?.saved_choice_availability;
  const read = receipt?.read_result, write = receipt?.write_result;
  const saved = open && Array.isArray(receipt?.saved) && receipt.saved.every((id: unknown) => typeof id === 'string') ? receipt.saved.length : undefined;
  useEffect(() => {
    if (!open) return;
    const session = client.session.getState();
    if (session.userId === null || reader !== requestReader(session.userId, session.readerVersion)) return;
    if (origin === undefined || availability === undefined || read === undefined || write === undefined ||
      (!hasGroups && (saved ?? 0) === 0 && write !== 'failed')) {
      pickerGroupReport.setState(null, true);
      return;
    }
    pickerGroupReport.setState({ reader, origin, current, saved, saved_choice_availability: availability, read_result: read, write_result: write }, true);
  }, [open, reader, origin, current, saved, availability, read, write, hasGroups]);
}
