/** What a template's spec says in a few words: where its image comes from, how it confines what runs in it, what it needs and what its exec node offers. The spec is the driver's own JSON, read with care.
 */

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const text = (value: unknown): string | null => (typeof value === 'string' && value !== '' ? value : null);
const count = (value: unknown): number | null => (typeof value === 'number' && Number.isFinite(value) ? value : null);

/** Where an image comes from: a Dockerfile, an archive of an image, or a reference to one. */
export type ImageSource = { kind: 'build' } | { kind: 'archive' } | { kind: 'image'; reference: string } | { kind: 'none' };

export function imageSource(spec: unknown): ImageSource {
  if (!isRecord(spec)) return { kind: 'none' };
  if (isRecord(spec.build)) return { kind: 'build' };
  if (isRecord(spec.archive)) return { kind: 'archive' };
  const reference = text(spec.image);
  return reference === null ? { kind: 'none' } : { kind: 'image', reference };
}

/** How a template confines what runs in it, as the facts its spec states: the container runtime, the network, a root that is read-only, privileges dropped. */
export interface Confinement {
  runtime: string | null;
  network: string | null;
  readOnlyRoot: boolean;
  noNewPrivileges: boolean;
}

export function confinementOf(spec: unknown): Confinement {
  const source = isRecord(spec) ? spec : {};
  const network = isRecord(source.network) ? source.network : {};
  const security = isRecord(source.security) ? source.security : {};
  return {
    runtime: text(source.runtime),
    network: text(network.mode),
    readOnlyRoot: security.read_only_rootfs === true,
    noNewPrivileges: security.no_new_privileges === true,
  };
}

/** The numbers a spec states about what a unit uses: the most it may use, the most its limits may be raised to, or what it needs at least. */
export interface SpecAmounts {
  cpuMillis: number | null;
  memoryMb: number | null;
  pids: number | null;
  diskGb: number | null;
}

/** A spec's `resources` (the limits), `resources_max` (how far they may be raised) or `needs` (what a host must have free). */
export function amountsOf(spec: unknown, member: 'resources' | 'resources_max' | 'needs'): SpecAmounts {
  const amounts = isRecord(spec) && isRecord(spec[member]) ? spec[member] : {};
  return { cpuMillis: count(amounts.cpu_millis), memoryMb: count(amounts.memory_mb), pids: count(amounts.pids), diskGb: count(amounts.disk_gb) };
}

/** Whether a spec states any of the numbers. */
export const hasAmounts = (amounts: SpecAmounts): boolean => Object.values(amounts).some((value) => value !== null);

/** What the template's exec node offers a harness that runs elsewhere, in the order it is tried. */
export function execOffers(spec: unknown): { offers: string[]; orchTools: boolean | null } {
  const exec = isRecord(spec) && isRecord(spec.exec) ? spec.exec : null;
  if (exec === null) return { offers: [], orchTools: null };
  const offers = Array.isArray(exec.offers) ? exec.offers.filter((entry): entry is string => typeof entry === 'string') : [];
  return { offers, orchTools: typeof exec.orch_tools === 'boolean' ? exec.orch_tools : null };
}

/** The spec as it is edited: indented JSON. */
export const specText = (spec: unknown): string => JSON.stringify(spec ?? {}, null, 2);

/** A spec read back from what was typed: an object, or nothing where the text is not one. */
export function readSpec(typed: string): Record<string, unknown> | null {
  try {
    const value: unknown = JSON.parse(typed);
    return isRecord(value) ? value : null;
  } catch {
    return null;
  }
}

/** One field a driver's schema describes at the top of a spec. */
export interface SchemaField {
  name: string;
  type: string;
  description: string;
  required: boolean;
}

/** The JSON Schema a driver declares for the specs of one kind of template, from the descriptor `GET /drivers` returns. */
export function specSchema(descriptor: unknown, kind: string): Record<string, unknown> | null {
  if (!isRecord(descriptor) || !isRecord(descriptor.spec_schemas)) return null;
  const schema = descriptor.spec_schemas[kind];
  return isRecord(schema) ? schema : null;
}

/** What a schema says of the type of a value: its `type`, or the shapes of an `enum`, or "any" where it says neither. */
function typeOf(schema: unknown): string {
  if (!isRecord(schema)) return 'any';
  if (typeof schema.type === 'string') return schema.type;
  if (Array.isArray(schema.type)) return schema.type.filter((entry): entry is string => typeof entry === 'string').join(' | ');
  if (Array.isArray(schema.enum)) return schema.enum.map(String).join(' | ');
  if (Array.isArray(schema.anyOf)) return schema.anyOf.map(typeOf).join(' | ');
  return 'any';
}

/** The fields at the top of a schema, in the order it names them. */
export function schemaFields(schema: Record<string, unknown> | null): SchemaField[] {
  if (schema === null || !isRecord(schema.properties)) return [];
  const required = Array.isArray(schema.required) ? schema.required.filter((entry): entry is string => typeof entry === 'string') : [];
  return Object.entries(schema.properties).map(([name, field]) => ({
    name,
    type: typeOf(field),
    description: isRecord(field) && typeof field.description === 'string' ? field.description : '',
    required: required.includes(name),
  }));
}

/**
 * A spec to start from: what a schema requires, and what it gives a default for, at the top. Where a
 * schema asks for nothing, an empty object; the person writes the rest.
 */
export function starterSpec(schema: Record<string, unknown> | null): Record<string, unknown> {
  const starter: Record<string, unknown> = {};
  if (schema === null || !isRecord(schema.properties)) return starter;
  const required = Array.isArray(schema.required) ? schema.required : [];
  for (const [name, field] of Object.entries(schema.properties)) {
    if (!isRecord(field)) continue;
    if ('default' in field && field.default !== null) starter[name] = structuredClone(field.default);
    else if (required.includes(name)) starter[name] = field.type === 'string' ? '' : field.type === 'object' ? {} : field.type === 'array' ? [] : null;
  }
  return starter;
}
