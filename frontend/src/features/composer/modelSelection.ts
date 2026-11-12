/** API selections keep presets and unrelated declared options while normalizing effort aliases. */
export interface ModelIdentity { provider_id: string; model: string }
export interface ModelSelection extends ModelIdentity { params?: unknown }
const record = (value: unknown): Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {};

/** Undefined follows the model start; null is an explicitly stored none. */
export function selectionEffort(params: unknown): string | null | undefined {
  const value = record(params);
  for (const effort of [value.effort, record(value.reasoning).effort, value.reasoning_effort, record(value.output_config).effort]) {
    if (effort !== undefined) return typeof effort === 'string' ? effort : null;
  }
  return undefined;
}

/** The server accepts the canonical effort field; remove conflicting wire aliases only. */
export function selectionWithEffort(params: unknown, effort: string): Record<string, unknown> {
  const { effort: _effort, reasoning_effort: _alias, ...next } = record(params);
  for (const key of ['reasoning', 'output_config']) {
    if (!Object.hasOwn(record(next[key]), 'effort')) continue;
    const { effort: _old, ...rest } = record(next[key]);
    if (Object.keys(rest).length === 0) Reflect.deleteProperty(next, key); else next[key] = rest;
  }
  return { ...next, effort };
}

export const sameSelectionModel = (left: ModelSelection | null | undefined, right: ModelSelection): boolean => left?.provider_id === right.provider_id && left.model === right.model;
