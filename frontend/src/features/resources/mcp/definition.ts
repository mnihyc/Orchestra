/** The ordinary MCP connection fields project the public definition without discarding Advanced settings. */
import type { TranslationKey } from '@/i18n';

export type Transport = 'stdio' | 'http';
export type Authentication = 'none' | 'bearer' | 'variables';
export type Definition = Record<string, unknown>;
export const BEARER_SECRET = 'MCP_BEARER_TOKEN';
const VARIABLE = /^[A-Za-z_][A-Za-z0-9_]*$/u;
const FIELDS = new Set(['transport', 'command', 'url', 'authentication', 'env', 'secret_env', 'working_dir', 'startup_timeout_secs', 'tools_allow', 'tools_deny', 'requires']);

function object(value: unknown): Definition {
  return typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Definition : {};
}
export function strings(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((entry: unknown): entry is string => typeof entry === 'string') : [];
}

export function connection(spec: Definition) {
  const transport: Transport = spec.transport === 'stdio' ? 'stdio' : 'http';
  const argv = strings(spec.command);
  const auth = object(spec.authentication);
  const secretNames = strings(spec.secret_env);
  const tokenSecret = typeof auth.secret === 'string' ? auth.secret : BEARER_SECRET;
  const authentication: Authentication = transport === 'http' ? auth.kind === 'bearer' ? 'bearer' : 'none' : secretNames.length > 0 ? 'variables' : 'none';
  return { transport, command: argv[0] ?? '', args: argv.slice(1), url: typeof spec.url === 'string' ? spec.url : '', authentication, secretNames, tokenSecret };
}

export function chooseTransport(spec: Definition, transport: Transport): Definition {
  const next: Definition = { ...spec, transport };
  delete next.args;
  if (transport === 'http') { delete next.command; next.url = typeof spec.url === 'string' ? spec.url : ''; }
  else { delete next.url; delete next.authentication; next.command = spec.transport === 'stdio' ? [connection(spec).command, ...connection(spec).args] : []; }
  return next;
}

export function chooseAuthentication(spec: Definition, mode: Authentication): Definition {
  const next = { ...spec };
  const current = connection(spec);
  if (mode === 'bearer') {
    next.authentication = { kind: 'bearer', secret: current.tokenSecret };
    next.secret_env = [...new Set([...current.secretNames, current.tokenSecret])];
  } else {
    delete next.authentication;
    next.secret_env = mode === 'variables' ? current.secretNames : current.transport === 'stdio' ? [] : current.secretNames.filter((name) => name !== current.tokenSecret);
  }
  return next;
}

export interface DefinitionIssue { field: 'definition' | 'command' | 'url' | 'authentication' | 'secretNames'; key: TranslationKey }
export function definitionIssue(spec: Definition | null): DefinitionIssue | null {
  if (spec === null || Object.keys(spec).some((name) => !FIELDS.has(name)) || spec.transport !== 'stdio' && spec.transport !== 'http') return { field: 'definition', key: 'resourceDetails.mcp.form.invalidDefinition' };
  if (spec.transport === 'stdio' && (!Array.isArray(spec.command) || spec.command.some((arg: unknown) => typeof arg !== 'string') || connection(spec).command.trim() === '')) return { field: 'command', key: 'resourceDetails.mcp.form.commandRequired' };
  if (spec.transport === 'http') {
    try {
      const url = new URL(connection(spec).url);
      if (!['http:', 'https:'].includes(url.protocol) || url.username !== '' || url.password !== '') throw new Error();
    } catch { return { field: 'url', key: 'resourceDetails.mcp.form.urlRequired' }; }
  }
  const names = strings(spec.secret_env);
  if (spec.secret_env !== undefined && (!Array.isArray(spec.secret_env) || names.length !== spec.secret_env.length || names.some((name) => !VARIABLE.test(name)) || new Set(names).size !== names.length)) return { field: 'secretNames', key: 'resourceDetails.mcp.form.invalidVariables' };
  if (spec.authentication !== undefined && spec.authentication !== null) {
    const auth = object(spec.authentication);
    if (spec.transport !== 'http' || auth.kind !== 'bearer' || typeof auth.secret !== 'string' || !names.includes(auth.secret)) return { field: 'authentication', key: 'resourceDetails.mcp.form.invalidAuthentication' };
  }
  return null;
}

/** Values typed in this visit are sent separately; blank fields retain saved values. */
export function secretWrites(spec: Definition, values: Readonly<Record<string, string>>): Record<string, string> {
  const writes: Record<string, string> = {};
  for (const name of strings(spec.secret_env)) {
    const value = values[name];
    if (value !== undefined && value !== '') writes[name] = value;
  }
  return writes;
}
