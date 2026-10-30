/** The fake driver's forms, complete examples and save/check refusals used by newcomer browser scenarios. */
type Shape = Record<string, unknown>;
const object = (value: unknown): Shape => typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Shape : {};
const field = (type: string, title: string, description: string, fallback: unknown = null, extra: Shape = {}): Shape => ({ type, title, description, default: fallback, ...extra });
const amounts = (): Shape => ({ type: 'object', title: 'Limits', description: 'Amounts use millicores, MiB, process counts and GB.', default: {}, additionalProperties: false, properties: {
  cpu_millis: field('integer', 'Processor time', 'Thousandths of a core; 4000 is four cores.', null, { minimum: 1, 'x-unit': 'millicores' }),
  memory_mb: field('integer', 'Memory', 'Memory in MiB.', null, { minimum: 1, 'x-unit': 'MiB' }),
  memory_swap_mb: field('integer', 'Memory and swap', 'Combined amount in MiB.', null, { minimum: 1, 'x-unit': 'MiB' }),
  pids: field('integer', 'Processes and threads', 'Combined process and thread count.', null, { minimum: 1 }),
  disk_gb: field('integer', 'Disk', 'Disk in GB.', null, { minimum: 1, 'x-unit': 'GB' }),
  blkio_weight: field('integer', 'Disk priority', 'Relative disk weight.', 500, { minimum: 10, maximum: 1000 }),
} });

export function templateSchema(kind: 'environment' | 'provider', base: Shape): Shape {
  const command = { type: 'object', additionalProperties: false, required: ['name', 'command'], properties: {
    name: field('string', 'Command name', 'A unique command name.', null, { minLength: 1 }),
    command: field('array', 'Program and arguments', 'Literal program followed by its arguments.', null, { minItems: 1, items: { type: 'string' } }),
    timeout_secs: field('integer', 'Time limit', 'Deadline in seconds.', 300, { minimum: 1, maximum: 3600, 'x-unit': 'seconds' }),
  } };
  const properties: Shape = { ...object(base.properties),
    image: field('string', 'Image reference', 'An image from a registry.', null, { minLength: 1, examples: ['node:22-bookworm'] }),
    image_digest: { anyOf: [{ type: 'string', pattern: '^sha256:[0-9a-f]{64}$' }, { type: 'null' }], title: 'Pinned image digest', description: 'Pin a particular image.', default: null },
    resources: amounts(),
    user: field('object', 'User', 'The image user or numeric IDs.', null, { properties: { name: field('string', 'User name', 'An account in the image.'), uid: field('integer', 'User ID', 'Numeric user ID.', null, { minimum: 0 }), gid: field('integer', 'Group ID', 'Numeric group ID.', null, { minimum: 0 }) } }),
    network: field('object', 'Network', 'How this unit reaches services.', {}, { properties: {
      mode: field('string', 'Network connection', 'Bridge or Orch alone.', 'bridge', { enum: ['bridge', 'none'] }),
      egress: field('string', 'Outgoing connections', 'Open or allow-listed.', 'open', { enum: ['open', 'restricted'] }),
      dns: field('array', 'DNS servers', 'Name server addresses.', [], { items: { type: 'string' } }),
    } }),
    security: field('object', 'Confinement', 'Container security options.', {}, { properties: {
      read_only_rootfs: field('boolean', 'Read-only image files', 'Prevent writes outside volumes.', false),
      no_new_privileges: field('boolean', 'Prevent extra privileges', 'Prevent gaining privileges.', false),
      cap_add: field('array', 'Added kernel capabilities', 'Linux capabilities added.', [], { items: { type: 'string' } }),
      cap_drop: field('array', 'Removed kernel capabilities', 'Linux capabilities removed.', [], { items: { type: 'string' } }),
    } }),
    env: field('object', 'Variables', 'Plain variables, never credentials.', {}, { additionalProperties: { type: 'string' } }),
    mounts: field('array', 'Extra storage', 'Extra volumes or temporary storage.', [], { items: { type: 'object', required: ['type', 'target'], properties: {
      type: field('string', 'Storage kind', 'Volume, tmpfs or bind.', null, { enum: ['volume', 'tmpfs', 'bind'] }),
      source: field('string', 'Storage source', 'Volume name or host path.'), target: field('string', 'Path inside the unit', 'An absolute path.', null, { pattern: '^/' }), read_only: field('boolean', 'Read-only storage', 'Prevent writes.', false),
    } } }),
    needs: { ...amounts(), title: 'Needs free', description: 'Free capacity for placement, without a reservation.' },
    host_labels: field('array', 'Required host labels', 'The host must carry all labels.', [], { items: { type: 'string' } }),
    after_up: field('array', 'Commands after start', 'Named commands before readiness.', [], { items: command }),
    init: { type: ['object', 'null'], title: 'Earlier startup command', description: 'Earlier command format.', default: null },
  };
  if (kind === 'environment') {
    properties.workspace_mount = field('string', 'Workspace path', 'Persistent workspace path.', '/workspace');
    properties.home_mount = field('string', 'Home path', 'Persistent home path.', '/home/agent');
    properties.resources_max = { ...amounts(), title: 'May be raised to' };
    properties.exec = field('object', 'Exec node', 'Tools offered to remote harnesses.', {}, { properties: {
      offers: field('array', 'Offered executors', 'Preference order.', [], { items: { type: 'string' } }), orch_tools: field('boolean', 'Offer Orch tools', 'Shared tools.', true),
    } });
    for (const name of ['base_capabilities', 'allowed_capabilities', 'colocated_harnesses', 'mcp_server_ids']) properties[name] = field('array', name, 'Requested tools or capabilities.', [], { items: { type: 'string' } });
    properties.sandbox = { type: 'object' }; properties.agent_defaults = { type: 'object' };
  } else {
    properties.plugins = field('array', 'Registered programs', 'Programs in the provider image.', [], { items: { type: 'object', additionalProperties: false, required: ['kind', 'name', 'command'], properties: {
      kind: field('string', 'Program kind', 'Connector interface.', null, { enum: ['provider', 'translator', 'adapter', 'executor'] }), name: field('string', 'Registered name', 'Unique program name.', null, { minLength: 1 }),
      command: field('array', 'Program and arguments', 'Literal program and arguments.', null, { minItems: 1, items: { type: 'string' } }),
      env: field('object', 'Program variables', 'Additional variables.', {}, { additionalProperties: { type: 'string' } }), restart: field('string', 'Restart policy', 'When an exited program restarts.', 'on-failure', { enum: ['never', 'on-failure', 'always'] }),
    } } });
  }
  const start = (image: string): Shape => ({ image, resources: { cpu_millis: 4000, memory_mb: 4096, pids: 4096 }, network: { mode: 'bridge', egress: 'open', dns: [] }, security: { no_new_privileges: true }, env: {}, mounts: [], host_labels: [], after_up: [], ...(kind === 'environment' ? { workspace_mount: '/workspace', home_mount: '/home/agent', exec: { offers: ['plain'], orch_tools: true } } : {}) });
  return { ...base, properties, 'x-orch-examples': [
    { id: 'node', title: 'Node web project', description: 'A Node workspace.', spec: start('node:22-bookworm') },
    { id: 'python-db', title: 'Python with a database beside it', description: 'Connect to a separately managed database.', spec: { ...start('python:3.13-bookworm'), env: { DATABASE_URL: 'postgresql://db.example.internal:5432/app' } } },
    { id: 'rust', title: 'Rust project', description: 'Cargo and the Rust compiler.', spec: start('rust:1-bookworm') },
    { id: 'kvm', title: 'Machine with KVM', description: 'A host with KVM and approval.', spec: { ...start('debian:bookworm'), host_labels: ['kvm'], ...(kind === 'environment' ? { base_capabilities: ['kvm'], allowed_capabilities: ['kvm'] } : {}) } },
    { id: 'restricted', title: 'Restricted outgoing connections', description: 'Use the place allow-list.', spec: { ...start('node:22-bookworm'), network: { mode: 'bridge', egress: 'restricted', dns: [] } } },
  ] };
}

/** The host example runs a real installed program with an existing account, rather than creating an image. */
export function hostTemplateSchema(kind: 'environment' | 'provider'): Shape {
  const shared = object(templateSchema(kind, {}).properties);
  return { type: 'object', required: ['run_as'], additionalProperties: true, properties: {
    run_as: field('string', 'Run as user', 'An existing account on the host.', null, { minLength: 1, examples: ['agent'] }),
    after_up: shared.after_up,
    ...(kind === 'provider' ? { plugins: shared.plugins } : {}),
  }, 'x-orch-examples': [{ id: 'host-account', name: 'Host', description: 'Run printf after start as the existing agent account.', spec: {
    run_as: 'agent', after_up: [{ name: '10-ready', command: ['/usr/bin/printf', 'Host tools are ready.'] }], ...(kind === 'provider' ? { plugins: [] } : {}),
  } }] };
}

/** The double covers the observable rules used here; the real API suite proves full JSON Schema parity. */
export function templateProblems(kind: 'environment' | 'provider', schema: Shape, spec: unknown): { pointer: string; message: string }[] {
  const errors: { pointer: string; message: string }[] = [];
  const problem = (pointer: string, message: string) => { errors.push({ pointer, message }); };
  const visit = (given: unknown, value: unknown, pointer: string) => {
    const shape = object(given);
    if (value === undefined) return;
    if (Array.isArray(shape.anyOf)) { if (value === null) return; visit(shape.anyOf[0], value, pointer); return; }
    const types = Array.isArray(shape.type) ? shape.type : [shape.type];
    const type = value === null ? 'null' : Array.isArray(value) ? 'array' : typeof value === 'number' && Number.isInteger(value) ? 'integer' : typeof value;
    if (shape.type !== undefined && !types.includes(type) && !(type === 'integer' && types.includes('number'))) { problem(pointer, 'the value has the wrong type'); return; }
    if (Array.isArray(shape.enum) && !shape.enum.includes(value)) problem(pointer, 'the value is not an allowed choice');
    if (typeof value === 'number' && (typeof shape.minimum === 'number' && value < shape.minimum || typeof shape.maximum === 'number' && value > shape.maximum)) problem(pointer, 'the number is outside the allowed range');
    if (typeof value === 'string' && (typeof shape.minLength === 'number' && value.length < shape.minLength || typeof shape.pattern === 'string' && !new RegExp(shape.pattern).test(value))) problem(pointer, 'the text does not have the required format');
    if (Array.isArray(value)) {
      if (typeof shape.minItems === 'number' && value.length < shape.minItems) problem(pointer, 'the list does not meet the required length');
      value.forEach((entry, index) => visit(shape.items, entry, `${pointer}/${index}`));
    } else if (value !== null && typeof value === 'object') {
      const fields = object(shape.properties);
      for (const name of Array.isArray(shape.required) ? shape.required : []) if (typeof name === 'string' && !(name in value)) problem(pointer, 'a required member is missing');
      for (const [key, entry] of Object.entries(object(value))) {
        const at = `${pointer}/${key.replaceAll('~', '~0').replaceAll('/', '~1')}`;
        if (fields[key] !== undefined) visit(fields[key], entry, at);
        else if (shape.additionalProperties === false) problem(pointer, 'the object contains a member this configuration does not accept');
        else if (shape.additionalProperties !== undefined) visit(shape.additionalProperties, entry, at);
      }
    }
  };
  visit(schema, spec, '');
  const value = object(spec);
  if (Object.hasOwn(object(schema.properties), 'image') && ['image', 'build', 'archive'].filter((key) => value[key] !== null && value[key] !== undefined).length !== 1) problem('', 'choose exactly one image, build or archive');
  const names = new Set<string>();
  for (const [index, entry] of (Array.isArray(value.after_up) ? value.after_up : []).entries()) {
    const row = object(entry);
    if (typeof row.name !== 'string' || row.name.trim() === '' || names.has(row.name)) problem(`/after_up/${index}/name`, 'command names must be nonempty and unique');
    if (typeof row.name === 'string') names.add(row.name);
    if (!Array.isArray(row.command) || typeof row.command[0] !== 'string' || row.command[0].trim() === '') problem(`/after_up/${index}/command`, 'a command needs a nonempty executable');
  }
  if (kind === 'environment' && value.plugins !== undefined) problem('/plugins', 'programs belong only to provider templates');
  return errors;
}
