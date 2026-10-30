/** Independently authorized reach projections never collide or refresh unrelated/hidden readers after a write. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { environmentReachQuery as peopleQuery } from '@/api/access';
import { agentReachKey } from '@/api/agentReach';
import { applyChanges, findAct } from '@/api/acts';
import { environmentReachQuery as agentsQuery } from '@/api/environments';
import type { AgentReachOverview, BrowserProfile, EnvironmentReach, NodeRef, ResolvedSetting } from '@/generated/api-types';
import { treeWriter } from '@/realtime/stores/tree';
import { detailEnvironment, detailHome } from '../component/resource-detail-fixtures';
import { ACME, acmeTree } from '../doubles/fixtures';

function reach(home: NodeRef = detailHome, name = 'BUILD_TOKEN_NAME'): AgentReachOverview {
  return { network: detailEnvironment.network, secrets: { names: [name], source: home }, mounts: [], credentials: [], browser_profiles: [{ name: 'work-profile' }] };
}
function setting(home: NodeRef = detailHome): ResolvedSetting {
  return { key: 'env.secret_names', value: ['DEPLOY_TOKEN_NAME'], own: { value: ['DEPLOY_TOKEN_NAME'], revision: 2, locked: false }, source: { scope: home.kind, node: home }, parts: [], locked_by: null, writable: true };
}

it('keeps the native people/permissions and agent/network replies in distinct cache entries', async () => {
  const cache = new QueryClient();
  const people: EnvironmentReach = { people: [], hidden: 7 }, overview = reach();
  const readPeople = vi.fn(() => Promise.resolve(people)), readAgents = vi.fn(() => Promise.resolve(overview));
  try {
    const peopleOptions = peopleQuery('workspace'), agentOptions = agentsQuery('workspace');
    expect(peopleOptions.queryKey).toEqual(['environment-reach', 'workspace']);
    expect(agentOptions.queryKey).toEqual(['agent-reach', 'workspace']);
    await Promise.all([cache.query({ ...peopleOptions, queryFn: readPeople }), cache.query({ ...agentOptions, queryFn: readAgents })]);
    expect(cache.getQueryData(peopleOptions.queryKey)).toEqual(people);
    expect(cache.getQueryData(agentOptions.queryKey)).toEqual(overview);
    expect(readPeople).toHaveBeenCalledTimes(1); expect(readAgents).toHaveBeenCalledTimes(1);
  } finally { cache.clear(); }
});

it('applies complete setting/profile answers and repairs only their shown native overview', async () => {
  const cache = new QueryClient(), id = detailEnvironment.id, key = agentReachKey(id);
  const old = reach(), renamed: BrowserProfile = { name: 'new-profile-name', browser: null, size_bytes: 128, used_at: null };
  const accepted = setting();
  cache.setQueryData(key, old);
  cache.setQueryData(['settings', detailHome.id], [{ ...accepted, value: old.secrets.names, own: null }]);
  cache.setQueryData(['environment-browsers', id], { profiles: [{ ...renamed, name: 'work-profile' }], browsers: [] });
  const read = vi.fn(() => Promise.resolve(reach(detailHome, 'DEPLOY_TOKEN_NAME')));
  const other = vi.fn(() => Promise.resolve(reach()));
  cache.setQueryData(agentReachKey('unrelated'), reach({ id: 'other-home', kind: 'org', name: 'Other', redacted: false }));
  const observer = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
  const unrelated = new QueryObserver(cache, { queryKey: agentReachKey('unrelated'), queryFn: other, staleTime: Infinity });
  const stops = [observer.subscribe(() => undefined), unrelated.subscribe(() => undefined)];
  const invalidate = vi.spyOn(cache, 'invalidateQueries'), refetch = vi.spyOn(cache, 'refetchQueries');
  try {
    await applyChanges(findAct('PUT', `/nodes/${detailHome.id}/settings/env.secret_names`, { input: { value: accepted.value }, answer: accepted })?.changes ?? [], cache);
    expect(cache.getQueryData(['settings', detailHome.id])).toEqual([accepted]);
    expect(cache.getQueryData(key)).toEqual(reach(detailHome, 'DEPLOY_TOKEN_NAME'));
    expect(read).toHaveBeenCalledTimes(1); expect(other).not.toHaveBeenCalled();
    const updated = { ...reach(detailHome, 'DEPLOY_TOKEN_NAME'), browser_profiles: [{ name: renamed.name }] };
    read.mockResolvedValueOnce(updated);
    await applyChanges(findAct('PATCH', `/environments/${id}/browser-profiles/work-profile`, { input: { name: renamed.name }, answer: renamed })?.changes ?? [], cache);
    expect(cache.getQueryData(['environment-browsers', id])).toEqual({ profiles: [renamed], browsers: [] });
    expect(cache.getQueryData(key)).toEqual(updated);
    expect(read).toHaveBeenCalledTimes(2); expect(other).not.toHaveBeenCalled();
    expect(refetch.mock.calls.map(([filter]) => filter)).toEqual([{ queryKey: key, exact: true }, { queryKey: key, exact: true }]);
    expect(invalidate).not.toHaveBeenCalled();
  } finally { for (const off of stops) off(); cache.clear(); }
});

it('expires a related hidden overview without reading it and leaves unrelated and withheld views alone', async () => {
  const cache = new QueryClient(), hidden = agentReachKey('hidden'), unrelated = agentReachKey('elsewhere'), withheld = agentReachKey('no-secret-right');
  cache.setQueryData(hidden, reach());
  const elsewhere = reach({ id: 'other-home', kind: 'org', name: 'Other', redacted: false });
  const cut = { ...reach(), secrets: { names: null, source: null } } satisfies AgentReachOverview;
  cache.setQueryData(unrelated, elsewhere); cache.setQueryData(withheld, cut);
  const readHidden = vi.fn(() => Promise.resolve(reach())), readOther = vi.fn(() => Promise.resolve(elsewhere)), readCut = vi.fn(() => Promise.resolve(cut));
  const paused = new QueryObserver(cache, { queryKey: hidden, queryFn: readHidden, enabled: false });
  const other = new QueryObserver(cache, { queryKey: unrelated, queryFn: readOther, staleTime: Infinity });
  const denied = new QueryObserver(cache, { queryKey: withheld, queryFn: readCut, staleTime: Infinity });
  const stops = [paused.subscribe(() => undefined), other.subscribe(() => undefined), denied.subscribe(() => undefined)];
  try {
    await applyChanges(findAct('PUT', `/nodes/${detailHome.id}/settings/env.secret_names`, { answer: setting() })?.changes ?? [], cache);
    expect(cache.getQueryData(hidden)).toBeUndefined();
    expect(cache.getQueryData(unrelated)).toEqual(elsewhere); expect(cache.getQueryData(withheld)).toEqual(cut);
    expect(readHidden).not.toHaveBeenCalled(); expect(readOther).not.toHaveBeenCalled(); expect(readCut).not.toHaveBeenCalled();
  } finally { for (const off of stops) off(); cache.clear(); }
});

it('uses the held native ancestry for an inherited write and supports an omitted legacy answer without guessing names', async () => {
  const cache = new QueryClient(), key = agentReachKey('workspace');
  const home: NodeRef = { id: ACME.fixLogin, kind: 'topic', name: 'Fix login', redacted: false };
  treeWriter.reset(ACME.org, acmeTree(), 1);
  cache.setQueryData(key, reach(home));
  const updated = reach(home, 'DEPLOY_TOKEN_NAME'), read = vi.fn(() => Promise.resolve(updated));
  const observer = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
  const off = observer.subscribe(() => undefined);
  try {
    await applyChanges(findAct('PUT', `/nodes/${ACME.infra}/settings/env.secret_names`)?.changes ?? [], cache);
    expect(read).not.toHaveBeenCalled();
    await applyChanges(findAct('PUT', `/nodes/${ACME.web}/settings/env.secret_names`)?.changes ?? [], cache);
    expect(read).toHaveBeenCalledTimes(1);
    expect(cache.getQueryData(key)).toEqual(updated);
    expect(cache.getQueryData(agentReachKey(ACME.web))).toBeUndefined();
  } finally { off(); cache.clear(); treeWriter.clear(); }
});

it('repairs an inherited setting from the overview native path while navigation has never loaded its tree', async () => {
  const cache = new QueryClient(), key = agentReachKey('workspace');
  treeWriter.clear();
  const home: NodeRef = { id: 'topic', kind: 'topic', name: 'Work', redacted: false };
  const parent: NodeRef = { id: 'project', kind: 'project', name: null, redacted: true };
  const before = { ...reach(home), home_path: [parent, home] } satisfies AgentReachOverview;
  const after = { ...before, secrets: { names: ['DEPLOY_TOKEN_NAME'], source: home } };
  cache.setQueryData(key, before);
  const read = vi.fn(() => Promise.resolve(after));
  const observer = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
  const off = observer.subscribe(() => undefined);
  try {
    await applyChanges(findAct('PUT', '/nodes/project/settings/env.secret_names', { answer: setting(parent) })?.changes ?? [], cache);
    expect(read).toHaveBeenCalledTimes(1);
    expect(cache.getQueryData(key)).toEqual(after);
    expect(cache.getQueryData(['node', home.id])).toBeUndefined();
    expect(cache.getQueryData(['environment', 'workspace'])).toBeUndefined();
    expect(cache.getQueryData(agentReachKey(parent.id))).toBeUndefined();
  } finally { off(); cache.clear(); treeWriter.clear(); }
});
