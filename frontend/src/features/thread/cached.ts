/** Cached views expose only reported capabilities and complete conversation frames to their screens. */
import type { Thread, ThreadCan } from '@/generated/api-types';

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** Missing or foreign cache fields never grant an action, and unknown is distinct from a refusal. */
export function threadCapabilities(value: unknown): Partial<ThreadCan> | null {
  if (!record(value) || !record(value.can)) return null;
  const reported: Partial<ThreadCan> = {};
  for (const key of ['archive', 'export', 'fork', 'place'] as const) if (typeof value.can[key] === 'boolean') reported[key] = value.can[key];
  return reported;
}

/** A thin reference or another act's DTO is not a conversation; retain the view and offer its reader. */
export function isThreadForScreen(value: unknown, id: string): value is Thread {
  if (!record(value) || value.id !== id || value.kind !== 'thread') return false;
  if (typeof value.name !== 'string' || typeof value.description !== 'string' || typeof value.org_id !== 'string'
    || typeof value.cwd !== 'string' || typeof value.directory !== 'string' || typeof value.archived !== 'boolean') return false;
  if (!['idle', 'starting', 'running', 'waiting_approval', 'blocked'].includes(String(value.state))) return false;
  return Array.isArray(value.path) && Array.isArray(value.environment_permissions)
    && record(value.owner) && typeof value.owner.display_name === 'string'
    && record(value.access) && Array.isArray(value.access.permissions)
    && record(value.input_access) && typeof value.input_access.allowed === 'boolean' && Array.isArray(value.input_access.missing)
    && (value.binding === null || record(value.binding) && record(value.binding.agent) && record(value.binding.provider) && record(value.binding.isolation));
}
