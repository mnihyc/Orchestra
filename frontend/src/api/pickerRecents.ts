/** One server preference read and atomic selection writer for every picker kind. */
import { queryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import type { MePickerRecentChoice, MePickerRecentReceipt, MePickerRecents } from '@/generated/api-types';

export const pickerRecentsKey = (person: string) => ['picker-recents', person] as const;
export const pickerRecentsQuery = (person: string) => queryOptions({
  queryKey: pickerRecentsKey(person),
  queryFn: ({signal}): Promise<MePickerRecents> => ok(api.GET('/me/picker-recents',{signal})),
  staleTime: 0,
  retry: false,
  refetchOnWindowFocus: false,
});

/** A model key includes its provider; names and access are never stored here. */
export const recordPickerChoice = (choice: MePickerRecentChoice): Promise<MePickerRecentReceipt> =>
  ok(api.POST('/me/picker-recents',{body:choice}));
