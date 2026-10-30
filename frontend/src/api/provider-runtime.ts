/** Managed provider runtime controls and the plan reviewed before a replacement. */
import { api, ok, repeatable } from './client';
import type { Job, Provider, ProviderUpdatePlan } from '@/generated/api-types';

export function startRuntime(id: string): Promise<Provider> {
  return ok(api.POST('/providers/{id}/start', { params: { path: { id } }, body: {} }));
}
export function stopRuntime(id: string, interrupt: boolean): Promise<Provider> {
  return ok(api.POST('/providers/{id}/stop', { params: { path: { id } }, body: { interrupt } }));
}
export function planRuntime(id: string, copyDirectories = false): Promise<ProviderUpdatePlan> {
  return ok(api.POST('/providers/{id}/runtime/updates/preflight', { params: { path: { id } }, body: { reason: 'update', copy_directories: copyDirectories } }));
}
export function updateRuntime(id: string, plan: string, key: string, notify?: boolean): Promise<Job> {
  return ok(api.POST('/providers/{id}/runtime/updates', { params: { path: { id } }, body: { preflight_id: plan, ...(notify===undefined?{}:{notify}) }, ...repeatable(key) }));
}
