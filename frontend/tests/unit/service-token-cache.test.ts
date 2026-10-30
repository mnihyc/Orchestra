/** Service-token acts update their own list and retake only the shown account's derived count. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';
import { serviceAccountsKey, serviceTokensKey } from '@/api/directory';
import type { ApiTokenInfo, ServiceAccount, TokenCreated } from '@/generated/api-types';
import { ACME, id } from '../doubles/fixtures';
import { World } from '../doubles/world';

for (const shown of [true, false]) it(`deletes an account with its list ${shown ? 'shown' : 'hidden'} without reading another org`, async () => {
  const world = new World(), person = world.people.get('ada')!;
  const accounts = () => structuredClone(world.settings.handle(person, 'GET', `/orgs/${ACME.org}/service-accounts`, new URLSearchParams(), {})!.body as ServiceAccount[]);
  const account = accounts()[0]!;
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  const key = serviceAccountsKey(ACME.org), otherKey = serviceAccountsKey('another-org');
  const other = [{ ...account, user: { ...account.user, id: id(0x99, 21) } }];
  cache.setQueryData(key, accounts()); cache.setQueryData(otherKey, other);
  const read = vi.fn(() => Promise.resolve(accounts())), readOther = vi.fn(() => Promise.resolve(other));
  const stop = shown ? new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined) : () => undefined;
  const stopOther = new QueryObserver(cache, { queryKey: otherKey, queryFn: readOther }).subscribe(() => undefined);
  try {
    world.settings.handle(person, 'DELETE', `/service-accounts/${account.user.id}`, new URLSearchParams(), {});
    await applyChanges(findAct('DELETE', `/service-accounts/${account.user.id}`)!.changes, cache);
    expect(cache.getQueryData(key)).toEqual(shown ? accounts() : undefined);
    expect(read).toHaveBeenCalledTimes(shown ? 1 : 0);
    expect(readOther).not.toHaveBeenCalled();
    expect(cache.getQueryData(otherKey)).toEqual(other);
  } finally { stop(); stopOther(); cache.clear(); }
});

for (const remove of [false, true]) for (const shown of [true, false]) it(`${remove ? 'revokes' : 'creates'} a service token with its account ${shown ? 'shown' : 'hidden'}, without reading another org`, async () => {
  const world = new World();
  const person = world.people.get('ada')!;
  const accounts = () => world.settings.handle(person, 'GET', `/orgs/${ACME.org}/service-accounts`, new URLSearchParams(), {})!.body as ServiceAccount[];
  const account = accounts()[0]!;
  const subject = account.user.id;
  const create = () => world.settings.handle(person, 'POST', `/service-accounts/${subject}/tokens`, new URLSearchParams(), { name: 'ci', proof: { password: person.password }, scopes: ['*'] })!.body as TokenCreated;
  const seed = remove ? create() : null;
  const before = accounts();
  const held = seed === null ? [] : [seed.info];
  const reply = remove ? undefined : create();
  if (remove) world.settings.handle(person, 'DELETE', `/service-accounts/${subject}/tokens/${seed!.info.id}`, new URLSearchParams(), {});
  const after = accounts();
  const tokens = remove ? [] : [reply!.info];
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  const otherKey = serviceAccountsKey('another-org');
  const accountKey = serviceAccountsKey(ACME.org);
  const tokenKey = serviceTokensKey(subject);
  const other = [{ ...account, user: { ...account.user, id: id(0x99, 20) } }];
  const readOther = vi.fn(() => Promise.resolve(other));
  const readAccount = vi.fn(() => Promise.resolve(accounts()));
  const readTokens = vi.fn(() => Promise.resolve(tokens));
  cache.setQueryData(otherKey, other);
  cache.setQueryData(accountKey, before);
  cache.setQueryData(tokenKey, held);
  const stopOther = new QueryObserver(cache, { queryKey: otherKey, queryFn: readOther }).subscribe(() => undefined);
  const stopAccount = shown ? new QueryObserver(cache, { queryKey: accountKey, queryFn: readAccount }).subscribe(() => undefined) : undefined;
  const stopTokens = new QueryObserver(cache, { queryKey: tokenKey, queryFn: readTokens }).subscribe(() => undefined);
  try {
    const act = findAct(remove ? 'DELETE' : 'POST', `/service-accounts/${subject}/tokens${remove ? `/${seed!.info.id}` : ''}`, { answer: reply });
    if (act === null) throw new Error('Missing service-token act');
    await applyChanges(act.changes, cache);
    expect(cache.getQueryData(accountKey)).toEqual(shown ? after : undefined);
    expect(readAccount).toHaveBeenCalledTimes(shown ? 1 : 0);
    expect(cache.getQueryData<ApiTokenInfo[]>(tokenKey)).toEqual(tokens);
    expect(readTokens).not.toHaveBeenCalled();
    expect(cache.getQueryData(otherKey)).toEqual(other);
    expect(readOther).not.toHaveBeenCalled();
  } finally { stopOther(); stopAccount?.(); stopTokens(); cache.clear(); }
});
