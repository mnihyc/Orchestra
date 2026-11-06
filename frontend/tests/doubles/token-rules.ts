/** Request-shape/catalogue parity for the UI double, not a replacement permission or key-use evaluator. */
import type { ApiTokenInfo, KeyAction, KeyOperation, KeyRule, KeyTargetKind } from '../../src/generated/api-types';
import { Refusal } from './world.ts';
import { KEY_TARGET_KINDS, TOKEN_OPERATIONS } from './token-operations.ts';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const invalid = (path: string): never => { throw new Refusal(400, 'request.invalid', 'the request is not valid', { errors: [{ path, message: 'is not a valid explicit key selection' }] }); };
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const object = (value: unknown, names: readonly string[], path: string): Record<string, unknown> => {
  if (!record(value) || Object.keys(value).some((name) => !names.includes(name))) return invalid(path);
  return value;
};
const kind = (value: unknown, path: string): KeyTargetKind => KEY_TARGET_KINDS.find((entry) => entry === value) ?? invalid(path);
const action = (value: unknown, path: string): KeyAction => value === 'read' || value === 'write' || value === 'manage' ? value : invalid(path);

function rulesOf(value: unknown): KeyRule[] {
  if (!Array.isArray(value)) return invalid('rules');
  const available = new Map(TOKEN_OPERATIONS.filter((entry) => entry.delegation === 'business').flatMap((entry) => entry.variants.map((variant) => [variant.operation, variant.facets] as const)));
  return value.map((raw: unknown, index) => {
    const at = `rules.${index}`, row = object(raw, ['target', 'resources', 'allow'], at), target = object(row.target, ['kind', 'id', 'reach'], `${at}.target`);
    const targetKind = kind(target.kind, `${at}.target.kind`), reach = target.reach;
    if (reach !== 'self' && reach !== 'subtree') return invalid(`${at}.target.reach`);
    if (reach === 'subtree' && !['org', 'project', 'topic', 'thread', 'document_container', 'account'].includes(targetKind)) return invalid(`${at}.target.reach`);
    const id = target.id;
    if (targetKind === 'system' ? (id !== undefined && id !== null) : typeof id !== 'string' || !UUID.test(id)) return invalid(`${at}.target.id`);
    if (!Array.isArray(row.allow) || row.allow.length === 0) return invalid(`${at}.allow`);
    const allow: KeyOperation[] = row.allow.map((rawSelection: unknown, selected) => {
      const point = `${at}.allow.${selected}`, selection = object(rawSelection, ['operation', 'actions'], point), operation = selection.operation;
      if (typeof operation !== 'string' || !available.has(operation)) return invalid(`${point}.operation`);
      if (!Array.isArray(selection.actions) || selection.actions.length === 0) return invalid(`${point}.actions`);
      const actions = selection.actions.map((item: unknown) => action(item, `${point}.actions`));
      if (actions.some((item) => !available.get(operation)?.some((facet) => facet.action === item))) return invalid(`${point}.actions`);
      return { operation, actions };
    });
    let resources: KeyTargetKind[] | null | undefined;
    if (row.resources !== undefined && row.resources !== null) {
      if (!Array.isArray(row.resources)) return invalid(`${at}.resources`);
      resources = row.resources.map((item: unknown) => kind(item, `${at}.resources`));
    } else resources = row.resources;
    return { target: { kind: targetKind, reach, ...(typeof id === 'string' ? { id } : {}) }, ...(resources === undefined ? {} : { resources }), allow };
  });
}

export function tokenScope(body: Record<string, unknown>): Pick<ApiTokenInfo, 'scopes' | 'rules'> {
  if ((body.rules !== undefined && body.rules !== null)) {
    if ((body.scopes !== undefined && body.scopes !== null) || (body.node_id !== undefined && body.node_id !== null)) return invalid('rules');
    return { scopes: [], rules: rulesOf(body.rules) };
  }
  if ((body.scopes === undefined || body.scopes === null) && (body.node_id === undefined || body.node_id === null)) return { scopes: [], rules: [] };
  if ((body.scopes !== undefined && body.scopes !== null) && (!Array.isArray(body.scopes) || body.scopes.some((item: unknown) => typeof item !== 'string'))) return invalid('scopes');
  return { scopes: Array.isArray(body.scopes) ? body.scopes.filter((item: unknown): item is string => typeof item === 'string') : [] };
}
