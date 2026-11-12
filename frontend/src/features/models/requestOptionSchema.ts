/** Catalogue edits touch one JSON Schema property; unknown protocols and raw keywords stay authored. */
type ObjectValue = Record<string, unknown>;
const object = (value: unknown): value is ObjectValue => typeof value === 'object' && value !== null && !Array.isArray(value);
const own = (value: ObjectValue, key: string): unknown => Object.hasOwn(value, key) ? value[key] : undefined;
const INDIRECT = ['$ref', 'allOf', 'anyOf', 'oneOf', 'not', 'if', 'then', 'else', 'const', 'enum'] as const;

/** These are authored rules, not claims about a model's native capability or an inherited layer. */
export interface OptionProperty {
  state: 'absent' | 'present' | 'blocked' | 'custom';
  rule?: unknown;
  required: boolean;
  closed: boolean;
}
export type SchemaEdit = { ok: true; value: ObjectValue } | { ok: false; reason: 'document' | 'path' | 'custom' | 'blocked' };

function container(value: unknown): 'editable' | 'blocked' | 'custom' {
  if (value === false) return 'blocked';
  if (value === undefined) return 'editable';
  if (!object(value) || INDIRECT.some((key) => Object.hasOwn(value, key))) return 'custom';
  if (value.type !== undefined && value.type !== 'object') return 'custom';
  return value.properties === undefined || object(value.properties) ? 'editable' : 'custom';
}

export function optionProperty(document: unknown, protocol: string, path: readonly string[]): OptionProperty {
  if (!object(document) || path.length === 0 || path.some((part) => part === '')) return { state: 'custom', required: false, closed: false };
  let schema = own(document, protocol);
  let closed = false;
  for (let index = 0; index < path.length; index += 1) {
    const status = container(schema);
    if (status !== 'editable') return { state: status === 'blocked' ? 'blocked' : 'custom', required: false, closed };
    const parent = object(schema) ? schema : {};
    const key = path[index];
    if (key === undefined) return { state: 'custom', required: false, closed };
    const properties = object(parent.properties) ? parent.properties : {};
    const present = Object.hasOwn(properties, key);
    const rule = own(properties, key);
    const required = Array.isArray(parent.required) && parent.required.includes(key);
    if (!present && parent.additionalProperties === false) closed = true;
    if (index === path.length - 1) return { state: present ? 'present' : 'absent', ...(present ? { rule } : {}), required, closed };
    schema = rule;
  }
  return { state: 'custom', required: false, closed };
}

/** Removing a leaf preserves its parent constraints, required list and every sibling. */
function change(schema: unknown, path: readonly string[], rule: unknown): SchemaEdit {
  const status = container(schema);
  if (status !== 'editable') return { ok: false, reason: status === 'blocked' ? 'blocked' : 'custom' };
  const parent = object(schema) ? schema : { type: 'object' };
  const properties = object(parent.properties) ? parent.properties : {};
  const key = path[0];
  if (key === undefined || key === '') return { ok: false, reason: 'path' };
  let value = rule;
  if (path.length > 1) {
    const before = own(properties, key);
    if (rule === undefined && before === undefined) return { ok: true, value: parent };
    const child = change(before, path.slice(1), rule);
    if (!child.ok) return child;
    value = child.value;
  }
  if (value === undefined && !Object.hasOwn(properties, key)) return { ok: true, value: parent };
  const next = { ...properties };
  if (value === undefined) Reflect.deleteProperty(next, key);
  else Object.defineProperty(next, key, { value, enumerable: true, configurable: true, writable: true });
  return { ok: true, value: { ...parent, properties: next } };
}

/** Undefined removes only this property's rule; false is an explicit rejection, never a reset. */
export function setOptionProperty(document: unknown, protocol: string, path: readonly string[], rule: unknown): SchemaEdit {
  if (!object(document)) return { ok: false, reason: 'document' };
  if (protocol === '' || path.length === 0 || path.some((part) => part === '')) return { ok: false, reason: 'path' };
  const before = own(document, protocol);
  if (before === undefined && rule === undefined) return { ok: true, value: document };
  const edited = change(before, path, rule);
  if (!edited.ok) return edited;
  return { ok: true, value: { ...document, [protocol]: edited.value } };
}
