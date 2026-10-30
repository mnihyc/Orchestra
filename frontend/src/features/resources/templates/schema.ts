/** Local schema navigation and lossless edits; validation belongs to the server's save-time checker. */
import { record } from '@/features/resources/UnitCommands';

export type Schema = Record<string, unknown>;
export interface SpecProblem { pointer: string; message: string }
export interface TemplateExample { id: string; name: string; description: string; spec: Record<string, unknown> }
/** Preserve structured values as JSON instead of JavaScript's object stringification. */
export const displayValue = (value: unknown): string => value === undefined ? '' : typeof value === 'string' ? value : JSON.stringify(value);
export const pointerPart = (part: string): string => part.replaceAll('~', '~0').replaceAll('/', '~1');
export const pointerParts = (pointer: string): string[] => pointer === '' ? [] : pointer.slice(1).split('/').map((part) => part.replaceAll('~1', '/').replaceAll('~0', '~'));

/** Resolve only local references; extension declarations cannot make the browser fetch a URL. */
export function resolveSchema(root: Schema, given: unknown, value?: unknown): Schema {
  let schema = record(given);
  const visited = new Set<string>();
  while (typeof schema.$ref === 'string' && schema.$ref.startsWith('#/') && !visited.has(schema.$ref)) {
    visited.add(schema.$ref);
    const referenced = pointerParts(schema.$ref.slice(1)).reduce<unknown>((node, part) => record(node)[part], root);
    const { $ref: _reference, ...own } = schema;
    schema = { ...record(referenced), ...own };
  }
  if (Array.isArray(schema.type)) {
    const types: unknown[] = schema.type;
    const actual = Array.isArray(value) ? 'array' : typeof value === 'number' && Number.isInteger(value) && types.includes('integer') ? 'integer' : typeof value;
    schema = { ...schema, type: types.includes(actual) && actual !== 'object' ? actual : types.find((type) => type !== 'null') };
  }
  const variants = schema.anyOf ?? schema.oneOf;
  if (Array.isArray(variants) && schema.properties === undefined) {
    const candidates = variants.map((variant) => resolveSchema(root, variant)).filter((variant) => variant.type !== 'null');
    const selected = candidates.find((candidate) => Array.isArray(candidate.required) && candidate.required.every((key) => typeof key === 'string' && key in record(value))) ?? candidates[0];
    if (selected !== undefined) schema = { ...schema, ...selected };
  }
  return schema;
}

/** New fields use declared defaults only; a null annotation means the member stays absent. */
export function newValue(root: Schema, given: unknown): unknown {
  const schema = resolveSchema(root, given);
  if (schema.default !== undefined && schema.default !== null) return structuredClone(schema.default);
  if (Array.isArray(schema.enum)) return schema.enum[0];
  if (schema.type === 'object' || schema.properties !== undefined) {
    return Object.fromEntries((Array.isArray(schema.required) ? schema.required : []).filter((key): key is string => typeof key === 'string').map((key) => [key, newValue(root, record(schema.properties)[key])]));
  }
  if (schema.type === 'array') return [];
  if (schema.type === 'boolean') return false;
  return '';
}

export function templateExamples(schema: Schema | null): TemplateExample[] {
  const offered = schema?.['x-orch-examples'];
  return Array.isArray(offered) ? offered.flatMap((value) => {
    const row = record(value);
    const name=typeof row.name==='string'?row.name:row.title;
    return typeof row.id === 'string' && row.id.trim()!=='' && typeof name === 'string' && name.trim()!=='' && typeof row.description === 'string' && row.description.trim()!=='' && row.spec !== null && typeof row.spec === 'object' && !Array.isArray(row.spec)
      ? [{ id: row.id, name, description: row.description, spec: record(row.spec) }] : [];
  }) : [];
}

/** Find a nested schema using the actual value so object alternatives keep their selected shape. */
export function schemaAt(root: Schema, parts: readonly string[], value?: unknown): Schema {
  let schema = root;
  let current = value;
  for (const part of parts) {
    schema = resolveSchema(root, schema, current);
    const properties = record(schema.properties);
    const alternatives = schema.anyOf ?? schema.oneOf;
    const alternate = Array.isArray(alternatives) ? alternatives.map((variant) => record(resolveSchema(root, variant).properties)).find((fields) => Object.hasOwn(fields, part))?.[part] : undefined;
    const next = schema.type === 'array' ? schema.items : Object.hasOwn(properties, part) ? properties[part] : alternate ?? schema.additionalProperties;
    schema = record(next);
    current = Array.isArray(current) ? current[Number(part)] : record(current)[part];
  }
  return resolveSchema(root, schema, current);
}

export function fieldPath(parts: readonly string[]): string {
  return parts.map((part) => /^\d+$/.test(part) ? '*' : part).join('.');
}

/** Only known keys are replaced; removing a field never strips its siblings. */
export function replaceMember(value: unknown, name: string, next: unknown): Record<string, unknown> {
  const updated = { ...record(value) };
  if (next === undefined) return Object.fromEntries(Object.entries(updated).filter(([key]) => key !== name));
  return { ...updated, [name]: next };
}

/** Read a field without treating an absent value as an explicit default. */
export function valueAt(value: unknown, parts: readonly string[]): unknown {
  return parts.reduce<unknown>((current, part) => Array.isArray(current) ? current[Number(part)] : record(current)[part], value);
}

/** A sparse field edit keeps siblings and plugin members; reset removes only the override. */
export function replaceAt(value: Record<string, unknown>, parts: readonly string[], next: unknown): Record<string, unknown> {
  const [name, ...rest] = parts;
  if (name === undefined) return value;
  if (rest.length === 0) return replaceMember(value, name, next);
  const child = replaceAt(record(value[name]), rest, next);
  return replaceMember(value, name, Object.keys(child).length === 0 ? undefined : child);
}
