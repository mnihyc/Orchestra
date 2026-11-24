import type { ResourceTarget } from '@/generated/api-types';

export const ENVIRONMENT_POOL_PREFIX = '\u0000pool:';

export function environmentTarget(value: string): ResourceTarget {
  if (value.startsWith(ENVIRONMENT_POOL_PREFIX)) {
    return { type: 'pool', kind: 'environment', id: value.slice(ENVIRONMENT_POOL_PREFIX.length) };
  }
  return { type: 'single', resource: { kind: 'environment', id: value } };
}

export function environmentValue(target: ResourceTarget): string | null {
  if (target.type === 'pool') return target.kind === 'environment' ? ENVIRONMENT_POOL_PREFIX + target.id : null;
  return target.resource.kind === 'environment' ? target.resource.id : null;
}

export function environmentPoolId(value: string | null): string | undefined {
  return value?.startsWith(ENVIRONMENT_POOL_PREFIX) ? value.slice(ENVIRONMENT_POOL_PREFIX.length) : undefined;
}
