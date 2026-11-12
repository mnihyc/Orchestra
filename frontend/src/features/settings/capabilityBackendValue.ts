/** Preserve other declared capabilities when a setting is shared, including the original web shape. */
import declarations from '../../../../shared/capabilities.json';
import type { BackendRef } from '@/generated/api-types';

export const objectValue = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
export function backendCapabilities(key: string): { id: string; title: string }[] {
  return Object.entries(declarations.capabilities).flatMap(([id, capability]) =>
    'backend_setting' in capability && capability.backend_setting === key ? [{ id, title: capability.title }] : []);
}
export const isCapabilityBackendSetting = (key: string): boolean => backendCapabilities(key).length > 0;

export function selectedBackend(value: unknown, capability: string): BackendRef | null {
  let chosen = value;
  while (objectValue(chosen) && objectValue(chosen.capabilities)) {
    chosen = Object.hasOwn(chosen.capabilities, capability) ? chosen.capabilities[capability] : chosen.default;
  }
  if (!objectValue(chosen)) return null;
  if (typeof chosen.route === 'string' && objectValue(chosen.offering) && typeof chosen.offering.provider === 'string' && typeof chosen.offering.offering === 'string') {
    return { ...chosen, route: chosen.route, config: chosen.config ?? null, offering: { provider: chosen.offering.provider, offering: chosen.offering.offering } };
  }
  // Display the existing web value through its declared route; no eligibility is computed here.
  if (typeof chosen.provider_id !== 'string') return null;
  const offering = chosen.kind === 'model' ? chosen.model_name : chosen.kind === 'api'
    ? capability === 'web_search' ? chosen.search_service : capability === 'page_fetch' ? chosen.fetch_service : null : null;
  if (typeof offering !== 'string') return null;
  const executor = chosen.kind === 'model' ? 'model-backed' : 'api-backed';
  const route = Object.entries(declarations.routes).find(([, route]) => 'executor' in route && route.executor === executor && route.capabilities.some((id: string) => id === capability));
  return route === undefined ? null : { route: route[0], config: null, offering: { provider: chosen.provider_id, offering } };
}

export function changeBackend(value: unknown, key: string, capability: string, backend: BackendRef | null): unknown {
  if (backendCapabilities(key).length === 1) return backend;
  if (objectValue(value) && objectValue(value.capabilities)) return { ...value, capabilities: { ...value.capabilities, [capability]: backend } };
  return { capabilities: { [capability]: backend }, ...(value === null || value === undefined ? {} : { default: value }) };
}
