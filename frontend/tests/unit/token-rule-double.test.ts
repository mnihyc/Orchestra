// @vitest-environment node
/** The browser fixture preserves canonical metadata and asked rules without inventing key-use authority. */
import { readFileSync } from 'node:fs';
import { expect, it } from 'vitest';
import type { KeyRule } from '../../src/generated/api-types';
import { TOKEN_OPERATIONS } from '../doubles/token-operations';
import { tokenScope } from '../doubles/token-rules';
import { ACME, id } from '../doubles/fixtures';
import { World } from '../doubles/world';

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const rules: KeyRule[] = [{ target: { kind: 'account', id: id(1, 1), reach: 'subtree' }, resources: ['key'], allow: [{ operation: 'MeTokenList', actions: ['read'] }] }];

it('matches its Business subset to public OpenAPI while contracts --check owns full native fixture parity', () => {
  const document: unknown = JSON.parse(readFileSync(new URL('../../../docs/openapi.json', import.meta.url), 'utf8'));
  if (!record(document) || !record(document.paths)) throw new Error('The genuine OpenAPI paths are missing');
  const metadata = Object.values(document.paths).flatMap((methods) => !record(methods) ? [] : Object.values(methods).flatMap((operation) => {
    if (!record(operation) || !record(operation['x-orch-operation'])) return [];
    return [operation['x-orch-operation']];
  })).sort((one, two) => String(one.operation_id) < String(two.operation_id) ? -1 : String(one.operation_id) > String(two.operation_id) ? 1 : 0);
  expect(TOKEN_OPERATIONS.filter((operation) => operation.delegation === 'business')).toEqual(metadata);
});

it('retains explicit rules without legacy wildcard fields, with detached request data', () => {
  const source = structuredClone(rules), selected = tokenScope({ rules: source });
  expect(selected).toEqual({ scopes: [], rules });
  source[0]!.allow[0]!.actions.push('write');
  expect(selected).toEqual({ scopes: [], rules });
});

it('defaults omitted and explicit empty modern scope to deny-all, preserving explicit legacy compatibility', () => {
  expect(tokenScope({})).toEqual({ scopes: [], rules: [] });
  expect(tokenScope({ rules: [] })).toEqual({ scopes: [], rules: [] });
  expect(tokenScope({ scopes: ['*'] })).toEqual({ scopes: ['*'] });
});

it('rejects mixed modes, unknown operation/actions and closed-entry additions', () => {
  expect(() => tokenScope({ rules, scopes: ['*'] })).toThrow();
  expect(() => tokenScope({ rules: [{ ...rules[0], hash: 'not-a-rule-member' }] })).toThrow();
  expect(() => tokenScope({ rules: [{ ...rules[0], allow: [{ operation: 'FutureOperation', actions: ['read'] }] }] })).toThrow();
  expect(() => tokenScope({ rules: [{ ...rules[0], allow: [{ operation: 'MeTokenList', actions: ['write'] }] }] })).toThrow();
});

it('mints the exact modern Info through both existing UI producers and retains a complete typed full response', () => {
  const world = new World(), person = world.people.get('ada')!;
  const answer = world.settings.handle(person, 'POST', '/me/tokens', new URLSearchParams(), { name: 'explicit', rules, proof: { password: person.password } });
  expect(answer?.body).toMatchObject({ already_created: false, info: { rules, scopes: [], node: null } });
  expect(world.settings.handle(person, 'GET', '/me/tokens', new URLSearchParams(), {})?.body).toEqual([expect.objectContaining({ rules, scopes: [] })]);
  const accounts = world.settings.handle(person, 'GET', `/orgs/${ACME.org}/service-accounts`, new URLSearchParams(), {})?.body;
  const account: unknown = Array.isArray(accounts) ? accounts[0] : undefined;
  if (!record(account) || !record(account.user) || typeof account.user.id !== 'string') throw new Error('The native-shaped service fixture is missing');
  const serviceId = account.user.id;
  const serviceRules = rules.map((rule) => ({ ...rule, target: { ...rule.target, id: serviceId } }));
  expect(world.settings.handle(person, 'POST', `/service-accounts/${serviceId}/tokens`, new URLSearchParams(), { name: 'service', rules: serviceRules, proof: { password: person.password } })?.body).toMatchObject({ already_created: false, info: { rules: serviceRules, scopes: [] } });
});

it('returns a secretless replay for the same actor/intent and refuses changed input', () => {
  const world = new World(), person = world.people.get('ada')!, body = { name: 'once', rules };
  const anyToken: unknown = expect.any(String);
  expect(world.settings.handle(person, 'POST', '/me/tokens', new URLSearchParams(), body, 'same-act')?.body).toMatchObject({ already_created: false, token: anyToken });
  expect(world.settings.handle(person, 'POST', '/me/tokens', new URLSearchParams(), { ...body, proof: { password: 'new proof' } }, 'same-act')?.body).toMatchObject({ already_created: true, token: null });
  expect(() => world.settings.handle(person, 'POST', '/me/tokens', new URLSearchParams(), { ...body, rules: [] }, 'same-act')).toThrow();
});
