/** Provider and thread ledger reads, native reset consumption and declared sign-in requests. */
import { queryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import type { CapStatus, LoginState, Provider, ProviderResetConsumeBody, ProviderResetConsumeReply } from '@/generated/api-types';
import type { PlanAnalytics } from '@/features/resources/providers/plan';
import { ledgerPeriods } from '@/features/resources/providers/plan';

/** One daily ledger query covers Today, Seven days and This month; Usage opens the same cached observation. */
export function providerLedgerQuery(id: string, now: number) {
  const { from, to } = ledgerPeriods(now);
  return queryOptions({ queryKey: ['usage', `provider:${id}`, from, to, 'day'] as const, retry: false,
    queryFn: async ({ signal }): Promise<PlanAnalytics> => ok(api.GET('/usage', { params: { query: { scope: `provider:${id}`, from, to, bucket: 'day', breakdowns: '' } }, signal })),
  });
}

/** The thread aggregate has no individual record, person breakdown or current-binding filter. */
export function threadLedgerQuery(id: string, createdAt: string, now: number) {
  const { to } = ledgerPeriods(now);
  const from = createdAt.slice(0, 10);
  return queryOptions({ queryKey: ['usage', `node:${id}`, from, to] as const, retry: false,
    queryFn: async ({ signal }): Promise<PlanAnalytics> => ok(api.GET('/usage', { params: { query: { scope: `node:${id}`, from, to, bucket: 'month', breakdowns: '' } }, signal })),
  });
}

export function planCapsQuery(id: string, provider: boolean) {
  return queryOptions({ queryKey: ['usage-caps', provider ? 'provider' : 'thread', id] as const, retry: false,
    queryFn: async ({ signal }): Promise<CapStatus[]> => provider
      ? ok(api.GET('/usage/caps', { params: { query: { provider_id: id } }, signal }))
      : (await ok(api.GET('/usage/limits', { params: { query: { node_id: id } }, signal }))).caps,
  });
}

export const loginStateQuery = (id: string) => queryOptions({ queryKey: ['provider-login', id] as const, retry: false,
  queryFn: ({ signal }): Promise<LoginState> => ok(api.GET('/providers/{id}/login', { params: { path: { id } }, signal })),
});

export function startProviderLogin(id: string, method: string, values: Record<string, string>): Promise<LoginState> {
  return ok(api.POST('/providers/{id}/login', { params: { path: { id } }, body: { method, values } }));
}
export function answerProviderLogin(id: string, values: Record<string, string>): Promise<LoginState> {
  return ok(api.POST('/providers/{id}/login/input', { params: { path: { id } }, body: { values } }));
}
export function cancelProviderLogin(id: string): Promise<LoginState> {
  return ok(api.POST('/providers/{id}/login/cancel', { params: { path: { id } } }));
}

export function consumeReset(id: string, body: ProviderResetConsumeBody): Promise<ProviderResetConsumeReply> {
  return ok(api.POST('/providers/{id}/resets/consume', { params: { path: { id } }, body }));
}

export function retryProvider(id: string): Promise<Provider> {
  return ok(api.POST('/providers/{id}/refresh', { params: { path: { id } }, body: { retry: true } }));
}

