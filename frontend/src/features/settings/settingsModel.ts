/** What the settings editor knows of a key and its value without drawing anything: the group a key belongs to, which keys a place has, where the value in force comes from, and what kind of control its schema asks for.
 *
 * The registry serves each key with the JSON Schema of its value. This reads that schema into a small
 * set of shapes (a switch, a whole number with a unit, one word of several, a text, a list, a map, an
 * object, anything else as JSON), so that the controls are drawn from what the server says and no
 * key is named here except where its words need a better arrangement than its schema gives.
 */
import type { SettingsTarget } from '@/api/settings';
import type { NodeKind, ResolvedSetting, SettingDef, SettingDef as SettingWithText } from '@/generated/api-types';

/** The groups of the settings screen, in the order they are shown: what a thread does first, what governs a place last. */
export const GROUP_ORDER = [
  'agent',
  'confinement',
  'models',
  'environment',
  'switching',
  'workspace',
  'collaboration',
  'chat_documents',
  'joining',
  'signing_in',
  'automation',
  'shells',
  'limits',
  'retention',
  'redaction',
  'provider_runtimes',
  'providers',
  'access',
  'system',
] as const;

export type SettingsGroup = (typeof GROUP_ORDER)[number];

export function isGroup(value: string): value is SettingsGroup {
  return (GROUP_ORDER as readonly string[]).includes(value);
}

/** What the first part of a key says of its group, for a registry that does not serve the group itself. */
const GROUP_OF_PREFIX: Readonly<Record<string, SettingsGroup>> = {
  agent: 'agent',
  orch_tools: 'agent',
  approval: 'confinement',
  sandbox: 'confinement',
  binding: 'confinement',
  capability: 'confinement',
  credential: 'confinement',
  models: 'models',
  env: 'environment',
  exec: 'environment',
  mcp: 'environment',
  thread: 'environment',
  switch: 'switching',
  workspace: 'workspace',
  storage: 'workspace',
  sharing: 'collaboration',
  collab: 'collaboration',
  broadcast: 'collaboration',
  chat: 'chat_documents',
  docs: 'chat_documents',
  invitations: 'joining',
  registration: 'joining',
  orgs: 'joining',
  auth: 'signing_in',
  auto: 'automation',
  terminal: 'shells',
  usage: 'limits',
  tags: 'limits',
  retention: 'retention',
  diagnostics: 'retention',
  redaction: 'redaction',
  runtime: 'provider_runtimes',
  providers: 'providers',
  resources: 'providers',
  templates: 'providers',
  access: 'access',
  hosts: 'system',
  mail: 'system',
  system: 'system',
  outbound: 'system',
};

/** A key as the editor handles it: the registry's own, with the group it is shown in. */
export interface KeyDef extends SettingWithText {
  group: SettingsGroup;
}

/** The group a key is shown in: the one the registry serves, else the one its name says, else the system's. */
export function groupOf(def: SettingDef): SettingsGroup {
  if (isGroup(def.group)) return def.group;
  const prefix = def.key.split('.')[0] ?? '';
  return GROUP_OF_PREFIX[prefix] ?? 'system';
}

export function withGroup(def: SettingDef): KeyDef {
  return { ...def, group: groupOf(def) };
}

/** The scope a node's own rows stand at. */
export function scopeOfNode(kind: NodeKind): string {
  return kind;
}

/** The keys that can be set at a scope, in the registry's own order. */
export function keysAt(defs: readonly KeyDef[], scope: string): KeyDef[] {
  return defs.filter((def) => def.scopes.includes(scope));
}

/** The keys by group, in the order of `GROUP_ORDER`; a group without keys is not listed. */
export function byGroup(defs: readonly KeyDef[]): { group: SettingsGroup; keys: KeyDef[] }[] {
  return GROUP_ORDER.map((group) => ({ group, keys: defs.filter((def) => def.group === group) })).filter((entry) => entry.keys.length > 0);
}

/** Whether a lock on a row of this key changes anything: where a lower scope can only narrow, it does not. */
export function honoursLocks(def: Pick<SettingDef, 'merge'>): boolean {
  return def.merge !== 'intersect' && def.merge !== 'min';
}

// ---------------------------------------------------------------- where the value in force comes from

export type Provenance =
  /** Nothing was set anywhere: what the system ships, or supplies. */
  | { kind: 'default'; scope: 'default' | 'environment' }
  /** Set in a place above, or in the system. */
  | { kind: 'inherited'; scope: string; place: string | null }
  /** Set in this place. */
  | { kind: 'here'; locked: boolean }
  /** Set in a place above that does not let it be changed here. */
  | { kind: 'locked'; scope: string; place: string | null };

/** The lock that decides a key at a place and lies above it. The API names the topmost lock on the way, which may be the row of the place itself: whoever makes the rules there changes that row, lock included, so a lock held here is not above here. */
export function lockAbove(setting: Pick<ResolvedSetting, 'locked_by'>, here: SettingsTarget): ResolvedSetting['locked_by'] {
  const lock = setting.locked_by;
  if (lock === null) return null;
  switch (here.kind) {
    case 'system':
      return null;
    case 'node':
      return lock.node?.id === here.id ? null : lock;
    default:
      // The own value of an environment or of a provider's runtime is the innermost scope: every lock on the way is above it.
      return lock;
  }
}

/** Where the value in force at a place comes from, from what the API says of the key. */
export function provenanceOf(setting: Pick<ResolvedSetting, 'own' | 'source' | 'locked_by'>, here: SettingsTarget): Provenance {
  const lock = lockAbove(setting, here);
  if (lock !== null) {
    return { kind: 'locked', scope: lock.scope, place: lock.node?.name ?? null };
  }
  if (setting.own !== null) return { kind: 'here', locked: setting.own.locked };
  const { scope, node } = setting.source;
  if (scope === 'default' || scope === 'environment') return { kind: 'default', scope };
  if (scope === 'environment_own' || scope === 'provider_own' || scope === 'host_own') return { kind: 'here', locked: false };
  return { kind: 'inherited', scope, place: node?.name ?? null };
}

// ---------------------------------------------------------------- what a value looks like

type Schema = Record<string, unknown>;

const isSchema = (value: unknown): value is Schema => typeof value === 'object' && value !== null && !Array.isArray(value);

/** How a value is edited: the shape the control is chosen by. */
export type Shape =
  | { type: 'bool' }
  | { type: 'int'; min: number | null; max: number | null; unit: string | null }
  | { type: 'choice'; values: readonly string[] }
  | { type: 'text'; maxLength: number | null; pattern: string | null }
  | { type: 'id' }
  | { type: 'time' }
  | { type: 'list'; item: Shape; unique: boolean }
  | { type: 'map'; value: Shape; byId: boolean }
  | { type: 'object'; members: readonly { name: string; shape: Shape; required: boolean }[]; open: boolean }
  | { type: 'nullable'; of: Shape }
  /** Anything this does not know how to draw: edited as JSON. */
  | { type: 'json' };

const numberOrNull = (value: unknown): number | null => (typeof value === 'number' ? value : null);

/** A served capability list narrows the registry's enum, including a set of enum members. */
export function availableShape(shape: Shape, available: readonly string[] | null | undefined): Shape {
  if (available === null || available === undefined) return shape;
  if (shape.type === 'choice') return { ...shape, values: shape.values.filter((value) => available.includes(value)) };
  if (shape.type === 'list') return { ...shape, item: availableShape(shape.item, available) };
  if (shape.type === 'nullable') return { ...shape, of: availableShape(shape.of, available) };
  return shape;
}

/** Reads a JSON Schema of the registry into a shape. */
export function shapeOf(schema: unknown): Shape {
  if (!isSchema(schema)) return { type: 'json' };
  const any = schema.anyOf;
  if (Array.isArray(any)) {
    const parts = any.filter(isSchema);
    const rest = parts.filter((part) => part.type !== 'null');
    // A value or nothing; anything else that is one of several kinds is edited as JSON.
    if (rest.length === 1 && rest.length < parts.length && rest[0] !== undefined) return { type: 'nullable', of: shapeOf(rest[0]) };
    return { type: 'json' };
  }
  switch (schema.type) {
    case 'boolean':
      return { type: 'bool' };
    case 'integer':
      return { type: 'int', min: numberOrNull(schema.minimum), max: numberOrNull(schema.maximum), unit: typeof schema['x-unit'] === 'string' ? schema['x-unit'] : null };
    case 'string': {
      if (Array.isArray(schema.enum)) return { type: 'choice', values: schema.enum.filter((entry): entry is string => typeof entry === 'string') };
      if (schema.format === 'uuid') return { type: 'id' };
      if (schema.format === 'date-time') return { type: 'time' };
      return { type: 'text', maxLength: numberOrNull(schema.maxLength), pattern: typeof schema.pattern === 'string' ? schema.pattern : null };
    }
    case 'array':
      return { type: 'list', item: shapeOf(schema.items), unique: schema.uniqueItems === true };
    case 'object': {
      if (isSchema(schema.properties)) {
        const required = Array.isArray(schema.required) ? schema.required : [];
        return {
          type: 'object',
          members: Object.entries(schema.properties).map(([name, member]) => ({ name, shape: shapeOf(member), required: required.includes(name) })),
          open: schema.additionalProperties !== false,
        };
      }
      if (isSchema(schema.additionalProperties)) {
        const names = schema.propertyNames;
        return { type: 'map', value: shapeOf(schema.additionalProperties), byId: isSchema(names) && names.format === 'uuid' };
      }
      return { type: 'json' };
    }
    default:
      return { type: 'json' };
  }
}

/** Whether two values are the same JSON, whatever order an object's members came in. */
export function sameValue(one: unknown, other: unknown): boolean {
  if (one === other) return true;
  if (Array.isArray(one) || Array.isArray(other)) {
    return Array.isArray(one) && Array.isArray(other) && one.length === other.length && one.every((entry, at) => sameValue(entry, other[at]));
  }
  if (isSchema(one) && isSchema(other)) {
    const keys = new Set([...Object.keys(one), ...Object.keys(other)]);
    return [...keys].every((key) => sameValue(one[key], other[key]));
  }
  return false;
}

/** A word of the registry made readable: `on_request` is "On request". */
export function words(value: string): string {
  const spaced = value.replaceAll('_', ' ').replaceAll('.', ' ');
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

/** The units a whole number can count, as the registry names them. */
export const UNIT_NAMES = ['milliseconds', 'seconds', 'minutes', 'hours', 'days', 'bytes', 'percent'] as const;

export type Unit = (typeof UNIT_NAMES)[number];

export function isUnit(value: string | null): value is Unit {
  return value !== null && (UNIT_NAMES as readonly string[]).includes(value);
}

/** The sizes a byte count is shown in, smallest first, each a multiple of 1024 of the one before. */
export const SIZE_UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB'] as const;

/** A byte count as a number and the largest size it is a whole number of: 67108864 is 64 MiB. */
export function sizeParts(bytes: number): { amount: number; unit: (typeof SIZE_UNITS)[number] } {
  let amount = bytes;
  let at = 0;
  while (at < SIZE_UNITS.length - 1 && amount !== 0 && amount % 1024 === 0) {
    amount /= 1024;
    at += 1;
  }
  return { amount, unit: SIZE_UNITS[at] ?? 'B' };
}

/** A number and a size back to bytes. */
export function bytesOf(amount: number, unit: (typeof SIZE_UNITS)[number]): number {
  return amount * 1024 ** SIZE_UNITS.indexOf(unit);
}

/**
 * What stands in the control when a key has no value at all: the sort of thing its shape starts from,
 * so that a person adds to nothing instead of to `undefined`.
 */
export function emptyOf(shape: Shape): unknown {
  switch (shape.type) {
    case 'bool':
      return false;
    case 'int':
      return shape.min ?? 0;
    case 'choice':
      return shape.values[0] ?? '';
    case 'text':
    case 'id':
    case 'time':
      return '';
    case 'list':
      return [];
    case 'map':
    case 'object':
      return {};
    case 'nullable':
      return null;
    case 'json':
      return null;
  }
}
