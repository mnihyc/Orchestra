/** Ordered joining and lock behavior used by the fake settings store. */

import type { SettingDef, SettingPart } from '../../src/generated/api-types.ts';

/** Resolves supplied parts; a higher lock keeps later parts visible but ignored, except for narrowing merges. */
export function resolvedParts(def: Pick<SettingDef, 'merge'>, supplied: SettingPart[]): { parts: SettingPart[]; value: unknown } {
  let locked = false;
  const parts = supplied.map((part) => {
    const ignored = locked && def.merge !== 'min' && def.merge !== 'intersect';
    if (part.locked && def.merge !== 'min' && def.merge !== 'intersect') locked = true;
    return { ...part, ignored };
  });
  let value: unknown = null;
  for (const part of parts.filter((one) => !one.ignored)) {
    const next = part.value;
    switch (def.merge) {
      case 'append':
        value = [typeof value === 'string' ? value : '', typeof next === 'string' ? next : ''].filter((text) => text !== '').join('\n\n');
        break;
      case 'union':
        value = [...new Map([...(Array.isArray(value) ? value as unknown[] : []), ...(Array.isArray(next) ? next as unknown[] : [])].map((item) => [JSON.stringify(item), item])).values()];
        break;
      case 'intersect':
        value = value === null ? next : next === null ? value : Array.isArray(value) && Array.isArray(next) ? value.filter((item) => next.some((other) => JSON.stringify(item) === JSON.stringify(other))) : [];
        break;
      case 'map':
        value = { ...(typeof value === 'object' && value !== null ? value : {}), ...(typeof next === 'object' && next !== null ? next : {}) };
        break;
      case 'min':
        value = typeof next !== 'number' ? value : typeof value === 'number' ? Math.min(value, next) : next;
        break;
      case 'replace':
        value = next;
    }
  }
  return { parts, value };
}

