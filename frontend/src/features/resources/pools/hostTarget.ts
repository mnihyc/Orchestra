/** Stored host intent has the same canonical identity as every other resource target. */
import type { ResourceTarget } from '@/generated/api-types';

export const HOST_POOL_PREFIX = '\u0000pool:';
export const singleHost = (id: string): ResourceTarget => ({ type: 'single', resource: { kind: 'host', id } });

/** Unknown setting values are not aliases for a physical host or an automatic choice. */
export function hostTarget(value: unknown): ResourceTarget | undefined {
  if (typeof value !== 'object' || value === null || !('type' in value)) return undefined;
  if (value.type === 'pool' && 'kind' in value && value.kind === 'host' && 'id' in value && typeof value.id === 'string') {
    return { type: 'pool', kind: 'host', id: value.id };
  }
  if (value.type === 'single' && 'resource' in value && typeof value.resource === 'object' && value.resource !== null
    && 'kind' in value.resource && value.resource.kind === 'host' && 'id' in value.resource && typeof value.resource.id === 'string') {
    return singleHost(value.resource.id);
  }
  return undefined;
}
export function singleHostId(value: unknown): string | undefined {
  const target = hostTarget(value);
  return target?.type === 'single' && target.resource.kind === 'host' ? target.resource.id : undefined;
}
export function hostPoolId(value: unknown): string | undefined {
  const target = hostTarget(value);
  return target?.type === 'pool' ? target.id : undefined;
}
export function hostChoiceTarget(value: string): ResourceTarget {
  return value.startsWith(HOST_POOL_PREFIX) ? { type: 'pool', kind: 'host', id: value.slice(HOST_POOL_PREFIX.length) } : singleHost(value);
}
