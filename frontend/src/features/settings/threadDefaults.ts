import { isRecord } from './draftChecks';

export const namedModel = (name: string) => ({ type: 'offering' as const, name });

export function defaultModelName(value: unknown): string | null {
  if (!isRecord(value)) return null;
  if (value.type === 'offering' && typeof value.name === 'string') return value.name;
  if (value.type !== 'single' || !isRecord(value.resource) || value.resource.kind !== 'model') return null;
  const backend = value.resource.backend;
  if (!isRecord(backend) || !isRecord(backend.offering)) return null;
  return typeof backend.offering.offering === 'string' ? backend.offering.offering : null;
}

export function defaultProviderId(value: unknown): string | null {
  if (!isRecord(value) || value.type !== 'single' || !isRecord(value.resource)) return null;
  return value.resource.kind === 'provider' && typeof value.resource.id === 'string' ? value.resource.id : null;
}

export function defaultPoolId(value: unknown, kind: 'provider' | 'model'): string | null {
  return isRecord(value) && value.type === 'pool' && value.kind === kind && typeof value.id === 'string' ? value.id : null;
}
