/** Each addressed list resumes its own device state; links and explicit resets replace it. */
import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { ReactNode } from 'react';
import { PageContext, type PageContextValue } from '@/features/workbench/pageContext';
import { client } from '@/api/client';
import { navigatedList, useListChoice, usePageListSearch } from '@/lib/listState';

const original=client.session.getState();
const validate=(state:Record<string,unknown>)=>Object.fromEntries(Object.entries(state).filter(([key])=>['view','sort','status','q','list','tab','actor','version','block','cursor'].includes(key)));
beforeEach(()=>{localStorage.clear();client.session.setState({userId:'list-reader'});});
afterEach(()=>{cleanup();client.session.setState(original);localStorage.clear();});

it('restores view, sort and filters independently for each page without restoring search words',()=>{
  const first=renderHook(()=>usePageListSearch('/project/$projectId/','/project/first',{view:'board',sort:'name',status:'done',q:'private search words'},validate));
  first.unmount();
  const second=renderHook(()=>usePageListSearch('/project/$projectId/','/project/second',{},validate));
  expect(second.result.current).not.toHaveProperty('view');second.unmount();
  const resumed=renderHook(()=>usePageListSearch('/project/$projectId/','/project/first',{},validate));
  expect(resumed.result.current).toMatchObject({view:'board',sort:'name',status:'done'});
  expect(resumed.result.current).not.toHaveProperty('q');
});

it('takes a link and Back state as a whole and remembers an explicitly cleared filter set',()=>{
  const initial=renderHook(()=>usePageListSearch('/everything/','/everything',{status:'disabled'},validate));initial.unmount();
  const linked=renderHook(()=>usePageListSearch('/everything/','/everything',{view:'mine',sort:'use',_list:true},validate));
  expect(linked.result.current).toMatchObject({view:'mine',sort:'use'});expect(linked.result.current).not.toHaveProperty('status');linked.unmount();
  const cleared=renderHook(()=>usePageListSearch('/everything/','/everything',{_list:true},validate));cleared.unmount();
  const resumed=renderHook(()=>usePageListSearch('/everything/','/everything',{},validate));
  expect(resumed.result.current).toEqual({_list:true});
});

it('clears local list controls with the existing device-data keys and keeps other lists independent',()=>{
  const one=renderHook(()=>useListChoice('first-list','sort','name'));act(()=>one.result.current[1]('use'));one.unmount();
  const other=renderHook(()=>useListChoice('second-list','sort','name'));expect(other.result.current[0]).toBe('name');other.unmount();
  const reopened=renderHook(()=>useListChoice('first-list','sort','name'));expect(reopened.result.current[0]).toBe('use');reopened.unmount();
  localStorage.clear();
  const reset=renderHook(()=>useListChoice('first-list','sort','name'));expect(reset.result.current[0]).toBe('name');
});

it('keeps content locators out of previous state and keeps a topic’s Settings filters apart from its content view',()=>{
  const document=renderHook(()=>usePageListSearch('/doc/$documentId/','/doc/one',{block:'old-block',version:4,cursor:'old-page',list:{'comments.show':'resolved'}},validate));document.unmount();
  const resumed=renderHook(()=>usePageListSearch('/doc/$documentId/','/doc/one',{},validate));
  expect(resumed.result.current).toMatchObject({list:{'comments.show':'resolved'}});expect(resumed.result.current).not.toHaveProperty('block');expect(resumed.result.current).not.toHaveProperty('version');expect(resumed.result.current).not.toHaveProperty('cursor');resumed.unmount();
  const content=renderHook(()=>usePageListSearch('/topic/$topicId/','/topic/one',{tab:'threads'},validate));content.unmount();
  const settings=renderHook(()=>usePageListSearch('/topic/$topicId/','/topic/one',{tab:'settings',actor:'person'},validate));settings.unmount();
  const topic=renderHook(()=>usePageListSearch('/topic/$topicId/','/topic/one',{},validate));expect(topic.result.current).toMatchObject({tab:'threads'});expect(topic.result.current).not.toHaveProperty('actor');topic.unmount();
  const settingsAgain=renderHook(()=>usePageListSearch('/topic/$topicId/','/topic/one',{tab:'settings'},validate));expect(settingsAgain.result.current).toMatchObject({tab:'settings',actor:'person'});
});

it('resumes a usage report’s controls but never its scope, so a bare address is the person’s own report',()=>{
  const keep=(state:Record<string,unknown>)=>Object.fromEntries(Object.entries(state).filter(([key])=>['scope','range','by'].includes(key)));
  const place=renderHook(()=>usePageListSearch('/usage/','/usage',{scope:'node:place-one',range:'7d',by:'person'},keep));
  expect(place.result.current).toMatchObject({scope:'node:place-one',range:'7d'});place.unmount();
  const own=renderHook(()=>usePageListSearch('/usage/','/usage',{},keep));
  expect(own.result.current).toMatchObject({range:'7d',by:'person'});expect(own.result.current).not.toHaveProperty('scope');own.unmount();
  // A scope an older visit left in the device's state is not resumed either.
  localStorage.clear();
  const stale=renderHook(()=>usePageListSearch('/usage/','/usage',{scope:'provider:old',range:'90d'},keep));stale.unmount();
  const again=renderHook(()=>usePageListSearch('/usage/','/usage',{},keep));
  expect(again.result.current).not.toHaveProperty('scope');
});

it('keeps unsigned list controls live without persisting another reader’s device choices',()=>{
  client.session.setState({userId:null});
  const choice=renderHook(()=>useListChoice('unsigned-list','kind','all'));
  act(()=>choice.result.current[1]('thread'));expect(choice.result.current[0]).toBe('thread');
  act(()=>choice.result.current[1]('all'));expect(choice.result.current[0]).toBe('all');
  expect(localStorage.length).toBe(0);
});
it('restores and clears nullable selected identities instead of comparing their type to null',()=>{
  const choice=renderHook(()=>useListChoice<string|null>('actor-list','actor',null));
  act(()=>choice.result.current[1]('selected-actor'));expect(choice.result.current[0]).toBe('selected-actor');choice.unmount();
  const restored=renderHook(()=>useListChoice<string|null>('actor-list','actor',null));expect(restored.result.current[0]).toBe('selected-actor');
  act(()=>restored.result.current[1](null));restored.unmount();
  expect(renderHook(()=>useListChoice<string|null>('actor-list','actor',null)).result.current[0]).toBeNull();
});
it('keeps nullable object selections live and clears anonymous state when the reader changes',()=>{
  client.session.setState({userId:null,readerVersion:1});
  const choice=renderHook(()=>useListChoice<{id:string}|null>('actor-list','person',null));
  act(()=>choice.result.current[1]({id:'selected-person'}));expect(choice.result.current[0]).toEqual({id:'selected-person'});
  act(()=>client.session.setState({userId:'another-reader',readerVersion:2}));expect(choice.result.current[0]).toBeNull();
  act(()=>client.session.setState({userId:null,readerVersion:3}));expect(choice.result.current[0]).toBeNull();
});

it('keeps settings and transient picker folds on this device without publishing a list address', () => {
  const navigate = vi.fn< PageContextValue['navigate'] >().mockResolvedValue(undefined);
  const target: PageContextValue['target'] = { route: '/org/$slug/settings/$section/', pathname: '/org/acme/settings/defaults', params: { slug: 'acme', section: 'defaults' }, search: {}, hash: '' };
  const wrapper = ({ children }: { children: ReactNode }) => <PageContext value={{ id: 'settings-page', target, navigate }}>{children}</PageContext>;
  const first = renderHook(() => useListChoice<string[]>('settings-picker', 'collapsedHomes', []), { wrapper });
  act(() => first.result.current[1](['home-one'])); expect(first.result.current[0]).toEqual(['home-one']);
  expect(navigate).not.toHaveBeenCalled(); first.unmount();
  const restored = renderHook(() => useListChoice<string[]>('settings-picker', 'collapsedHomes', []), { wrapper });
  expect(restored.result.current[0]).toEqual(['home-one']); restored.unmount();
  target.route = '/usage/'; target.pathname = '/usage'; target.search = { _list: true, list: { 'usage-picker.collapsedHomes': ['from-link'] } };
  const transient = renderHook(() => useListChoice<string[]>('usage-picker', 'collapsedHomes', [], 'device'), { wrapper });
  expect(transient.result.current[0]).toEqual([]); act(() => transient.result.current[1](['home-two']));
  expect(transient.result.current[0]).toEqual(['home-two']); expect(navigate).not.toHaveBeenCalled();
});
it('puts a list control into the address while it is on screen, and never the default of a drawer that is closed', () => {
  const navigate = vi.fn<PageContextValue['navigate']>().mockResolvedValue(undefined);
  const target: PageContextValue['target'] = { route: '/doc/$documentId/', pathname: '/doc/one', params: { documentId: 'one' }, search: { block: 'found' }, hash: '' };
  const wrapper = ({ children }: { children: ReactNode }) => <PageContext value={{ id: 'document-page', target, navigate }}>{children}</PageContext>;
  const drawer = renderHook(({ open }) => useListChoice<string>('document-history', 'author', 'all', 'address', open), { wrapper, initialProps: { open: false } });
  expect(drawer.result.current[0]).toBe('all');
  expect(navigate).not.toHaveBeenCalled();
  drawer.rerender({ open: true });
  const search = navigate.mock.calls[0]?.[0].search;
  if (typeof search !== 'function') throw new Error('An open drawer publishes its list state');
  expect(search(target.search)).toEqual({ block: 'found', _list: true, list: { 'document-history.author': 'all' } });
});
// The rule of the address: the choices of the controls on the screen it names, each when chosen or shown, always with the marker; no other screen's.
const topicSettings = (section: string): PageContextValue['target'] => ({ route: '/topic/$topicId/', pathname: '/topic/one', params: { topicId: 'one' }, search: { tab: 'settings', section }, hash: '' });
function addressOf(navigate: ReturnType<typeof vi.fn<PageContextValue['navigate']>>, target: PageContextValue['target']): Record<string, unknown> {
  // What the page's navigations wrote one after another into its address.
  return navigate.mock.calls.reduce<Record<string, unknown>>((address, [options]) => typeof options.search === 'function' ? options.search(address) : address, { ...target.search });
}

it('takes an act of the finder to a screen with a list without any choice of another screen, and with the marker', () => {
  const navigate = vi.fn<PageContextValue['navigate']>().mockResolvedValue(undefined);
  // The act pushes a bare address; the topic page reads the usage board's choices for what it asks, and the audit list shows its own.
  const target = topicSettings('audit');
  const wrapper = ({ children }: { children: ReactNode }) => <PageContext value={{ id: 'topic-page', target, navigate }}>{children}</PageContext>;
  const arrived = renderHook(() => ({
    period: useListChoice<string>('usage-board', 'period', 'week', 'address', false),
    other: useListChoice<string>('usage-board', 'other', 'model', 'address', false),
    group: useListChoice<string>('trace', 'group', 'day'),
    filters: useListChoice<boolean>('trace', 'filters', true),
  }), { wrapper });
  expect(arrived.result.current.period[0]).toBe('week');
  const address = addressOf(navigate, target);
  expect(address).toEqual({ tab: 'settings', section: 'audit', _list: true, list: { 'trace.group': 'day', 'trace.filters': true } });
  // A choice made on the screen goes in beside them, and the marker stays.
  act(() => arrived.result.current.group[1]('month'));
  expect(addressOf(navigate, target)).toMatchObject({ _list: true, list: { 'trace.group': 'month', 'trace.filters': true } });
  expect(addressOf(navigate, target).list).not.toHaveProperty('usage-board.period');
});

it('carries no choice of the screen it leaves, and only the controls on the screen it stays on', () => {
  const navigate = vi.fn<PageContextValue['navigate']>().mockResolvedValue(undefined);
  const target = topicSettings('audit');
  const wrapper = ({ children }: { children: ReactNode }) => <PageContext value={{ id: 'carrying-page', target, navigate }}>{children}</PageContext>;
  const screen = renderHook(() => useListChoice<string>('trace', 'group', 'day'), { wrapper });
  const route = '/topic/$topicId/';
  const left = { tab: 'settings', section: 'audit', _list: true, list: { 'trace.group': 'week', 'usage-board.period': 'month' } };
  // To a screen of the same page with none of those controls: nothing goes along, whether the caller spread the search or not.
  expect(navigatedList(route, 'carrying-page', left, { ...left, section: 'access' })).toBeUndefined();
  expect(navigatedList(route, 'carrying-page', left, { tab: 'threads' })).toBeUndefined();
  // The same screen: what is on it, and not what some other screen put there.
  expect(navigatedList(route, 'carrying-page', left, { ...left, q: 'x' })).toEqual({ 'trace.group': 'week' });
  // A list that the navigation names is the target screen's own, as is its remembered list.
  expect(navigatedList(route, 'carrying-page', left, { tab: 'threads', list: { 'topic-threads.sort': 'name' } })).toEqual({ 'topic-threads.sort': 'name' });
  // A page that has no control on screen holds no list.
  expect(navigatedList(route, 'another-page', left, { ...left })).toBeUndefined();
  screen.unmount();
  expect(navigatedList(route, 'carrying-page', left, { ...left })).toBeUndefined();
});

it('keeps the settings locator but omits internal list metadata from a plain org settings address', () => {
  const addressed = renderHook(() => usePageListSearch('/org/$slug/settings/$section/', '/org/acme/settings/defaults', { setting: 'agent.model' }, (value) => ({ setting: value.setting })));
  expect(addressed.result.current).toEqual({ setting: 'agent.model' });
});

// EN: Providers → choose a view/group/order → fold a group → reload; only readable choices are shared.
// 中文：提供方 → 选择视图、分组与排序 → 折叠分组 → 刷新；地址只保留可读选择。
it('shares resource view choices as plain parameters while collapsed groups stay on the device', () => {
  const navigate = vi.fn<PageContextValue['navigate']>().mockResolvedValue(undefined);
  const target: PageContextValue['target'] = { route: '/resources/providers/', pathname: '/resources/providers', params: {}, search: { view: 'viewable', sort: 'name', group: 'home' }, hash: '' };
  const wrapper = ({ children }: { children: ReactNode }) => <PageContext value={{ id: 'providers-page', target, navigate }}>{children}</PageContext>;
  const view = renderHook(() => useListChoice<string>('resources.providers', 'view', 'usable'), { wrapper });
  expect(view.result.current[0]).toBe('viewable'); expect(navigate).not.toHaveBeenCalled();
  act(() => view.result.current[1]('mine'));
  const search = navigate.mock.calls[0]?.[0].search;
  if (typeof search !== 'function') throw new Error('The view updates its own readable search');
  expect(search(target.search)).toEqual({ view: 'mine', sort: 'name', group: 'home' });
  navigate.mockClear();
  const folds = renderHook(() => useListChoice<string[]>('resources.providers', 'collapsedGroups.home', []), { wrapper });
  act(() => folds.result.current[1](['home-one'])); expect(navigate).not.toHaveBeenCalled(); folds.unmount();
  const restored = renderHook(() => useListChoice<string[]>('resources.providers', 'collapsedGroups.home', []), { wrapper });
  expect(restored.result.current[0]).toEqual(['home-one']);
});
it('returns a readable resource address without publishing bulk state or a marker', () => {
  const view = renderHook(() => usePageListSearch('/resources/providers/', '/resources/providers', { view: 'mine', group: 'owner', sort: 'use', direction: 'desc', _list: true, list: { 'resources.providers.collapsedGroups.home': ['hidden-fold'] } }, (state) => ({ view: state.view })));
  expect(view.result.current).toEqual({ view: 'mine', group: 'owner', sort: 'use', direction: 'desc' });
});
