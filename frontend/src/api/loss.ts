/** Reviewed permanent loss and independent environment recovery through the common job machinery. */
import { queryOptions } from '@tanstack/react-query';
import { api, ok, repeatable } from '@/api/client';
import type { EnvironmentBatch, EnvironmentRecoverPreflightBody, EnvironmentRecoveryPlan, ForkCreateBody, LossDeclareBody, LossPlan, RecoverySources, LossTarget, OperationRecipients, HostDeletePreflightBody, HostDeletePlan, HostDeleteCreateBody, Job } from '@/generated/api-types';

export const lossPlanQuery = (target: LossTarget) => queryOptions({ queryKey: ['loss-plan', target], queryFn: ({ signal }): Promise<LossPlan> => ok(api.POST('/loss/preflight', { body: { target }, signal })), staleTime: 0, gcTime: 0, retry: false });
export function declareLost(body: LossDeclareBody, key: string): Promise<LossPlan> { return ok(api.POST('/loss', { body, ...repeatable(key) })); }
export const recoverableQuery = (target: LossTarget) => queryOptions({ queryKey: ['recoverable', target], queryFn: ({ signal }): Promise<RecoverySources> => ok(api.POST('/loss/recovery', { body: { target }, signal })) });
export function planRecovery(body: EnvironmentRecoverPreflightBody, signal?: AbortSignal): Promise<EnvironmentRecoveryPlan> { return ok(api.POST('/environments/recover/preflight', { body, ...(signal === undefined ? {} : { signal }) })); }
export function startRecovery(body: ForkCreateBody, key: string): Promise<EnvironmentBatch> { return ok(api.POST('/environments/recover', { body, ...repeatable(key) })); }
export const recoveryBatchQuery = (batch_id: string) => queryOptions({ queryKey: ['recovery-batch', batch_id], queryFn: ({ signal }): Promise<EnvironmentBatch> => ok(api.GET('/environments/recover/{batch_id}', { params: { path: { batch_id } }, signal })) });

export const operationRecipientsQuery = (id: string) => queryOptions({ queryKey: ['operation-recipients', id], queryFn: ({ signal }): Promise<OperationRecipients> => ok(api.GET('/operations/{id}/recipients', { params: { path: { id } }, signal })), staleTime: 0, retry: false });
export function moreOperationRecipients(id: string, cursor?: string, signal?: AbortSignal): Promise<OperationRecipients> { return ok(api.GET('/operations/{id}/recipients', { params: { path: { id }, query: cursor === undefined ? {} : { cursor } }, ...(signal === undefined ? {} : { signal }) })); }

export function planHostDelete(id: string, body: HostDeletePreflightBody, signal?: AbortSignal): Promise<HostDeletePlan> { return ok(api.POST('/hosts/{id}/delete/preflight', { params: { path: { id } }, body, ...(signal === undefined ? {} : { signal }) })); }
export function startHostDelete(id: string, body: HostDeleteCreateBody, key: string): Promise<Job> { return ok(api.POST('/hosts/{id}/delete', { params: { path: { id } }, body, ...repeatable(key) })); }
