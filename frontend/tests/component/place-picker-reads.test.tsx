/** A closed destination picker owns no tree read; opening it retains live choices and native filtering. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, screen, within, waitFor, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Nodes from '@/api/nodes';
import { orgsKey, type TreeReply } from '@/api/nodes';
import { PlacePicker, placesOfTree } from '@/features/settings/PlacePicker';
import { i18n, setLocale } from '@/i18n';
import { socket } from '@/realtime/live';
import { hierarchyFeed } from '@/api/hierarchyFeed';
import type { FeedListener } from '@/realtime/socket';
import { treeWriter, useTreeStore } from '@/realtime/stores/tree';
import { ACME, acmeTree, me, org, userRef } from '../doubles/fixtures';
import { show,skipOverflowFallback } from './support';

const reads = vi.hoisted(() => ({ tree: vi.fn<(id:string,selection:Nodes.TreeSelection,signal?:AbortSignal)=>Promise<TreeReply>>() }));
skipOverflowFallback();
vi.mock('@/api/nodes', async (load) => ({ ...await load<typeof Nodes>(), fetchTreePage: reads.tree }));
afterEach(async () => { cleanup(); treeWriter.clear(); vi.clearAllMocks(); await setLocale('en'); });

function page(nodes:Nodes.TreeReply['nodes'],selection:Nodes.TreeSelection):Nodes.TreeReply {
  const paths=placesOfTree(nodes,' / ',true),needle=selection.q?.trim().toLocaleLowerCase()??'';
  const matched=nodes.filter((node)=>(selection.kind===undefined||node.kind===selection.kind)&&(paths.find((path)=>path.id===node.id)?.path.toLocaleLowerCase().includes(needle)??false));
  return {nodes:matched,ancestors:nodes.filter((node)=>!matched.includes(node)),hidden:[],seq:1,since:null,complete:true};
}

for (const locale of ['en', 'zh-CN'] as const) {
  it(`reads destinations only on opening and keeps the same choices on reopening in ${locale}`, async () => {
    await setLocale(locale);
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
    cache.setQueryData(orgsKey, [org(ACME.org, 'acme', 'Acme', userRef(me(1, 'ada', 'Ada')))]);
    const reply: TreeReply = { nodes: acmeTree(), hidden: [], seq: 1, since: null, complete: true };
    reads.tree.mockImplementation((_id:string,selection:Nodes.TreeSelection)=>Promise.resolve(page(reply.nodes,selection)));
    const changed = vi.fn(), user = userEvent.setup();
    show(<QueryClientProvider client={cache}><PlacePicker label={i18n.t('mcpSwitch.place')} value={null} onChange={changed} includeThreads /></QueryClientProvider>);
    expect(reads.tree).not.toHaveBeenCalled();
    const picker = screen.getByRole('combobox', { name: i18n.t('mcpSwitch.place') });
    await user.click(picker);
    await screen.findByRole('treeitem', { name: 'Acme / Research / Web / Fix login' });
    expect(reads.tree).toHaveBeenCalledTimes(1);
    await user.keyboard('{Escape}');
    await waitFor(()=>expect(cache.getQueriesData({queryKey:['tree-places']})).toHaveLength(0));
    await user.click(picker);
    await user.click(await screen.findByRole('treeitem', { name: 'Acme / Research / Web / Fix login' }));
    expect(changed).toHaveBeenCalledWith(expect.objectContaining({ id: ACME.fixLogin, kind: 'topic' }));
    expect(reads.tree).toHaveBeenCalledTimes(2);
  });

  it(`reads a bounded selector page and preserves the root and kind restrictions in ${locale}`, async () => {
    await setLocale(locale);
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    cache.setQueryData(orgsKey, [org(ACME.org, 'acme', 'Acme', userRef(me(1, 'ada', 'Ada')))]);
    treeWriter.reset(ACME.org, acmeTree(), 1);
    reads.tree.mockImplementation((_id:string,selection:Nodes.TreeSelection)=>Promise.resolve(page(acmeTree(),selection)));
    const changed = vi.fn(), user = userEvent.setup();
    show(<QueryClientProvider client={cache}><PlacePicker label={i18n.t('mcpSwitch.place')} value={null} onChange={changed} rootOrgId={ACME.org} only="topic" /></QueryClientProvider>);
    expect(reads.tree).not.toHaveBeenCalled();
    await user.click(screen.getByRole('combobox', { name: i18n.t('mcpSwitch.place') }));
    expect(await screen.findByRole('treeitem', { name: 'Acme / Research / Web / Fix login' })).toBeVisible();
    expect(screen.queryByRole('treeitem', { name: /^Acme \/ Web$/ })).not.toBeInTheDocument();
    expect(reads.tree).toHaveBeenCalledTimes(1);
    expect(reads.tree.mock.calls[0]?.[1]).toMatchObject({kind:'topic'});
  });
}

it('chooses through the shared hierarchy with folding, keyboard search and no first-page cutoff', async () => {
  await setLocale('en');
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(orgsKey, [org(ACME.org, 'acme', 'Acme', userRef(me(1, 'ada', 'Ada')))]);
  const base = acmeTree().find((node) => node.id === ACME.org);
  if (base === undefined) throw new Error('The fixture has a root.');
  const nodes = [base, ...Array.from({ length: 45 }, (_, index) => ({ ...base, id: `project-${index}`, kind: 'project' as const, name: `Project ${String(index).padStart(2, '0')}`, parent_id: base.id }))];
  treeWriter.reset(ACME.org, nodes, 1);
  reads.tree.mockImplementation((_id:string,selection:Nodes.TreeSelection)=>Promise.resolve(page(nodes,selection)));
  const changed = vi.fn(), user = userEvent.setup();
  show(<QueryClientProvider client={cache}><PlacePicker label="Place" value={null} onChange={changed} /></QueryClientProvider>);
  await user.click(screen.getByRole('combobox', { name: 'Place' }));
  const tree = await screen.findByRole('tree', { name: 'Place' });
  expect(within(tree).getAllByRole('treeitem')).toHaveLength(46);
  expect(within(tree).getByRole('treeitem', { name: 'Acme / Project 44' })).toBeVisible();
  const root = within(tree).getByRole('treeitem', { name: 'Acme' });
  root.focus();
  await user.keyboard('{ArrowLeft}');
  expect(within(tree).getAllByRole('treeitem')).toHaveLength(1);
  await user.keyboard('{ArrowRight}');
  expect(within(tree).getAllByRole('treeitem')).toHaveLength(46);
  await user.type(screen.getByRole('searchbox', { name: i18n.t('settings.place.placeholder') }), 'Project 44');
  const searched=await screen.findByRole('tree',{name:'Place'});
  await waitFor(()=>expect(within(searched).getAllByRole('treeitem')).toHaveLength(2));
  await user.keyboard('{ArrowDown}{End}{Enter}');
  expect(changed).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ id: 'project-44' }));
  expect(reads.tree.mock.calls.some(([,selection])=>selection.q==='Project 44')).toBe(true);
});

it.each(['en','zh-CN'] as const)('finds a destination never retained by navigation or the first picker page in %s',async(locale)=>{
  await setLocale(locale);const cache=new QueryClient({defaultOptions:{queries:{retry:false}}});
  cache.setQueryData(orgsKey,[org(ACME.org,'acme','Acme',userRef(me(1,'ada','Ada')))]);
  const root=acmeTree().find((row)=>row.id===ACME.org);if(root===undefined)throw new Error('The fixture needs its root');
  const first={...root,id:'first-place',kind:'project' as const,name:'First place',parent_id:root.id};
  const late={...first,id:'never-retained',name:'Never loaded'};
  reads.tree.mockImplementation((_id,selection)=>Promise.resolve(page(selection.q===''?[root,first]:[root,late],selection)));
  const changed=vi.fn(),user=userEvent.setup();
  show(<QueryClientProvider client={cache}><PlacePicker label="Place" value={null} onChange={changed}/></QueryClientProvider>);
  await user.click(screen.getByRole('combobox',{name:'Place'}));
  await screen.findByRole('treeitem',{name:'Acme / First place'});
  expect(screen.queryByRole('treeitem',{name:'Acme / Never loaded'})).not.toBeInTheDocument();
  expect(useTreeStore.getState().byOrg.get(ACME.org)?.nodes.has(late.id)).not.toBe(true);
  fireEvent.change(screen.getByRole('searchbox'),{target:{value:'Never loaded'}});
  const found=await screen.findByRole('treeitem',{name:'Acme / Never loaded'});found.focus();await user.keyboard('{Enter}');
  expect(changed).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({id:late.id}));
  expect(reads.tree.mock.calls.at(-1)?.[1]).toMatchObject({q:'Never loaded'});
});


it.each(['en','zh-CN'] as const)('keeps a native admitted destination whose intermediate parent is not disclosed in %s',async(locale)=>{
  await setLocale(locale);const cache=new QueryClient({defaultOptions:{queries:{retry:false}}});
  cache.setQueryData(orgsKey,[org(ACME.org,'acme','Acme',userRef(me(1,'ada','Ada')))]);
  const root=acmeTree().find((row)=>row.id===ACME.org);if(root===undefined)throw new Error('The fixture needs its root');
  const leaf={...root,id:'shared-behind-private',kind:'project' as const,parent_id:'undisclosed-parent',name:'Shared project'};
  reads.tree.mockResolvedValue({nodes:[leaf],ancestors:[root],hidden:[],seq:1,since:null,complete:true,next_cursor:null});
  const choose=vi.fn();const user=userEvent.setup();
  show(<QueryClientProvider client={cache}><PlacePicker label="Place" value={null} onChange={choose}/></QueryClientProvider>);
  await user.click(screen.getByRole('combobox',{name:'Place'}));
  const found=await screen.findByRole('treeitem',{name:'Shared project'});await user.click(found);
  expect(choose).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({id:leaf.id,rootOrgId:ACME.org,path:'Shared project'}));
  expect(screen.queryByText('undisclosed-parent')).not.toBeInTheDocument();
});


it.each(['en','zh-CN'] as const)('keeps snapshot choices and says when their live source waits for capacity in %s',async(locale)=>{
  await setLocale(locale);const cache=new QueryClient({defaultOptions:{queries:{retry:false}}});
  cache.setQueryData(orgsKey,[org(ACME.org,'acme','Acme',userRef(me(1,'ada','Ada')))]);
  let listener:FeedListener|undefined;
  const subscribe=vi.spyOn(socket,'subscribe').mockImplementation((_feed,next)=>{listener=next;return {unsubscribe:()=>undefined,updateViews:()=>undefined};});
  reads.tree.mockResolvedValue({...page(acmeTree(),{q:''}),feed:hierarchyFeed()});
  const user=userEvent.setup();
  show(<QueryClientProvider client={cache}><PlacePicker label="Place" value={null} onChange={()=>undefined} /></QueryClientProvider>);
  await user.click(screen.getByRole('combobox',{name:'Place'}));
  await screen.findByRole('treeitem',{name:'Acme / Research / Web / Fix login'});
  expect(screen.getByText(i18n.t('navigation.pages.snapshot'))).toBeVisible();
  await waitFor(()=>expect(listener).toBeDefined());
  const window=listener?.hierarchy?.()[0]?.window;
  listener?.onFrame({op:'error',id:1,code:'too_many_subscriptions',detail:'The connection is full'});
  expect(await screen.findByText(i18n.t('navigation.pages.waitingForLive'))).toBeVisible();
  expect(screen.getByRole('treeitem',{name:'Acme / Research / Web / Fix login'})).toBeVisible();
  listener?.onFrame({op:'event',id:1,feed:hierarchyFeed(),data:{window,hierarchy:{seq:2,nodes:acmeTree(),ancestors:[],complete:true}}});
  await waitFor(()=>expect(screen.queryByText(i18n.t('navigation.pages.waitingForLive'))).not.toBeInTheDocument());
  cleanup();subscribe.mockRestore();
});
