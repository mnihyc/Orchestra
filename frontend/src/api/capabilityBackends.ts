/** The ordinary scoped read supplies current choices; the client never resolves eligibility. */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import type { CapabilityBackendCandidate, Provider } from '@/generated/api-types';

export interface BackendPage { items: CapabilityBackendCandidate[]; next_cursor: string | null }

export function capabilityBackendsQuery(reader: string, provider: string, place: string, capability: string, q: string) {
  return infiniteQueryOptions({
    queryKey: ['capability-backends', reader, provider, place, capability, q] as const,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ signal, pageParam }): Promise<BackendPage> => ok(api.GET('/providers/{id}/capability-backends', {
      params: { path: { id: provider }, query: { capability, place, q, ...(pageParam === undefined ? {} : { cursor: pageParam }) } }, signal,
    })),
    getNextPageParam: (last: BackendPage) => last.next_cursor ?? undefined,
    retry: false,
  });
}

export function selectedCapabilityBackendQuery(reader: string, provider: string, place: string, capability: string, offering: string) {
  return queryOptions({
    queryKey: ['selected-capability-backend', reader, provider, place, capability, offering] as const,
    queryFn: ({ signal }): Promise<BackendPage> => ok(api.GET('/providers/{id}/capability-backends', {
      params: { path: { id: provider }, query: { capability, place, offering } }, signal,
    })),
    retry: false,
  });
}

export function capabilityProviderQuery(reader: string, provider: string) {
  return queryOptions({
    queryKey: ['capability-backend-provider', reader, provider] as const,
    queryFn: ({ signal }): Promise<Provider | null> => ok(api.GET('/providers/{id}', { params: { path: { id: provider } }, signal })),
    retry: false,
  });
}
