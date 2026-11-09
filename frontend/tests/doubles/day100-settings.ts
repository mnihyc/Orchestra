/** Ordered resolution used by the native-shaped fake settings store. */
import type { SettingDef, SettingPart, SettingSource } from '../../src/generated/api-types.ts';

export function narrowingMerge(mode: SettingDef['merge']): boolean {
  return ['min', 'max', 'all', 'any', 'stricter', 'intersect'].includes(mode);
}
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
function numeric(value: unknown): value is number | string {
  return typeof value === 'number' && Number.isFinite(value) || typeof value === 'string' && /^[0-9]+(?:\.[0-9]+)?$/.test(value);
}
function compare(first: number | string, second: number | string): number {
  const [a = '0', af = ''] = String(first).split('.'), [b = '0', bf = ''] = String(second).split('.');
  if (![a, af, b, bf].every((part) => /^[0-9]*$/.test(part))) return Number(first) - Number(second);
  const places = Math.max(af.length, bf.length), scale = 10n ** BigInt(places);
  const one = BigInt(a) * scale + BigInt(af.padEnd(places, '0') || '0');
  const two = BigInt(b) * scale + BigInt(bf.padEnd(places, '0') || '0');
  return one < two ? -1 : one > two ? 1 : 0;
}

/** A tightening merge ignores locks and uses a shipped default only when no stored scope sets it. */
export function resolvedParts(def: Pick<SettingDef, 'merge'> & Partial<Pick<SettingDef, 'display' | 'schema'>>, supplied: SettingPart[]): { parts: SettingPart[]; value: unknown; source: SettingSource } {
  const mode = def.merge, narrowing = narrowingMerge(mode);
  let locked = false;
  const parts = supplied.map((part) => {
    const ignored = locked && !narrowing;
    if (part.locked && !narrowing) locked = true;
    return { ...part, ignored };
  });
  const active = parts.filter((part) => !part.ignored);
  const stored = active.filter((part) => part.source.scope !== 'default' && part.source.scope !== 'environment');
  const counted = narrowing ? stored.length > 0 ? stored : active.slice(-1) : active;
  let value: unknown = null, source: SettingSource = { scope: 'default', node: null };
  const unlimited = (next: unknown) => next === null || def.display?.zero === 'no_limit' && numeric(next) && compare(next, 0) === 0;
  const choices = record(def.schema) && Array.isArray(def.schema.enum) ? def.schema.enum : [];
  for (const part of counted) {
    const next = part.value;
    switch (mode) {
      case 'append':
        value = [typeof value === 'string' ? value : '', typeof next === 'string' ? next : ''].filter((text) => text !== '').join('\n\n'); source = part.source;
        break;
      case 'union':
        value = [...new Map([...(Array.isArray(value) ? value : []), ...(Array.isArray(next) ? next : [])].map((item) => [JSON.stringify(item), item])).values()]; source = part.source;
        break;
      case 'intersect': {
        if (!Array.isArray(next)) break;
        if (value === null) { value = [...next]; source = part.source; break; }
        if (!Array.isArray(value)) break;
        const kept: unknown[] = value.filter((item) => next.some((other) => JSON.stringify(item) === JSON.stringify(other)));
        if (kept.length !== value.length || next.every((item) => kept.some((other) => JSON.stringify(item) === JSON.stringify(other)))) source = part.source;
        value = kept;
        break;
      }
      case 'map':
        value = { ...(record(value) ? value : {}), ...(record(next) ? next : {}) }; source = part.source;
        break;
      case 'min':
      case 'max':
        if (unlimited(next)) { if (value === null) source = part.source; break; }
        if (numeric(next) && (!numeric(value) || (mode === 'min' ? compare(next, value) <= 0 : compare(next, value) >= 0))) { value = next; source = part.source; }
        break;
      case 'all':
      case 'any':
        if (typeof next === 'boolean' && (typeof value !== 'boolean' || next === (mode === 'any') || next === value)) { value = next; source = part.source; }
        break;
      case 'stricter':
        if (typeof next === 'string' && choices.includes(next) && (value === null || choices.indexOf(next) >= choices.indexOf(value))) { value = next; source = part.source; }
        break;
      case 'replace':
        value = next; source = part.source;
    }
  }
  return { parts, value, source };
}
