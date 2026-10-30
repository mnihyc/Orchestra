/** Change effects follow the current driver; memory warnings compare the draft to measured use. */
import type { RuntimeChanges } from '@/generated/api-types';

export type RuntimeEffect = 'liveResources' | 'recreateResources' | 'pendingResources' | 'liveMounts' | 'recreate' | 'driverUnknown';
export function runtimeEffect(key: string, runtime: RuntimeChanges | null | undefined): RuntimeEffect | null {
  if ((key === 'env.resources' || key === 'runtime.resources') && runtime?.pending_reason) return 'pendingResources';
  if (key === 'env.resources' || key === 'runtime.resources') return runtime === null || runtime === undefined ? 'driverUnknown' : runtime.live_changes.resources ? 'liveResources' : 'recreateResources';
  if (key === 'env.mounts') return runtime === null || runtime === undefined ? 'driverUnknown' : runtime.live_changes.mounts ? 'liveMounts' : 'recreate';
  if (['env.template_id', 'env.runtime', 'runtime.template_id', 'runtime.host_id', 'env.storage', 'env.vars', 'env.secrets', 'runtime.vars', 'runtime.secrets'].includes(key)) return 'recreate';
  return null;
}

export function memoryBelowUse(draft: unknown, usedBytes: number | null | undefined): boolean {
  if (usedBytes === null || usedBytes === undefined || typeof draft !== 'object' || draft === null) return false;
  const memory = (draft as Record<string, unknown>).memory_mb;
  return typeof memory === 'number' && memory > 0 && memory * 1024 * 1024 < usedBytes;
}
