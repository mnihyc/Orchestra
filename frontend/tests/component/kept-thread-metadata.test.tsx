/** A visible inactive tab observes only native metadata while its closed parent's rows stay released. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Nodes from '@/api/nodes';
import { client } from '@/api/client';
import { socket } from '@/realtime/live';
import { hierarchyFeed } from '@/api/hierarchyFeed';
import type { FeedListener } from '@/realtime/socket';
import { treeWriter,useTreeStore } from '@/realtime/stores/tree';
import { LiveThreadStateIcon } from '@/features/workbench/ThreadIcon';
import { ViewTab } from '@/ui/ViewTab';
import { ACME,acmeTree } from '../doubles/fixtures';
import { i18n } from '@/i18n';
import { display } from '../doubles/thread-display';
import { show } from './support';

const calls=vi.hoisted(()=>({pages:vi.fn<(root:string,selection:Nodes.TreeSelection,signal?:AbortSignal)=>Promise<Nodes.TreeReply>>()}));
vi.mock('@/api/nodes',async(load)=>({...await load<typeof Nodes>(),fetchTreePage:calls.pages}));
const before=client.session.getState(),status=socket.status.getState();
let listener:FeedListener|undefined;
beforeEach(()=>{
  calls.pages.mockReset();client.session.setState({status:'signed_in',userId:'reader',readerVersion:(client.session.getState().readerVersion??0)+1});
  socket.status.setState({phase:'ready'});
  vi.spyOn(socket,'subscribe').mockImplementation((_feed,next)=>{listener=next;return {unsubscribe:()=>undefined,updateViews:()=>undefined};});
});
afterEach(()=>{cleanup();treeWriter.clear();vi.restoreAllMocks();client.session.setState(before);socket.status.setState(status);listener=undefined;});

it('finishes an inactive kept tabs icon from metadata after its parent closes without reading hidden content',async()=>{
  const row=acmeTree().find((node)=>node.id===ACME.mainThread)!;
  const running=display('running',{seq:10}),finished=display('finished',{seq:20});
  const nodes=acmeTree().map((node)=>node.id===row.id?{...row,thread:{...row.thread!,display:running}}:node);
  treeWriter.reset(ACME.org,nodes,10);
  calls.pages.mockResolvedValue({nodes,ancestors:[],hidden:[],seq:10,since:null,complete:true,feed:hierarchyFeed()});
  show(<ViewTab id="kept" panelId="hidden-content" title="Work" active={false} tabbable menu={[]} onSelect={()=>undefined} onClose={()=>undefined}
    icon={<LiveThreadStateIcon display={running} threadId={row.id} rootId={ACME.org}/>}/>);
  await waitFor(()=>expect(listener).toBeDefined());
  const window=listener?.hierarchy?.()[0]?.window;
  act(()=>treeWriter.releaseBranch(ACME.org,ACME.web));expect(useTreeStore.getState().byOrg.get(ACME.org)?.nodes.has(row.id)).toBe(false);
  act(()=>listener?.onFrame({op:'event',id:1,feed:hierarchyFeed(),data:{window,hierarchy:{seq:20,complete:true,nodes:[{...row,thread:{...row.thread!,display:finished}}],ancestors:[]}}}));
  expect(await screen.findByRole('img',{name:/Finished/})).toBeVisible();
  expect(calls.pages).toHaveBeenCalledTimes(1);expect(calls.pages.mock.calls[0]?.[1]).toEqual({path_to:row.id});
  expect(useTreeStore.getState().byOrg.get(ACME.org)?.nodes.has(row.id)).toBe(false);
  act(()=>listener?.onFrame({op:'event',id:1,feed:hierarchyFeed(),data:{window,problem:'authz.denied',hierarchy:{seq:21,complete:true,nodes:[],ancestors:[]}}}));
  expect(await screen.findByRole('img',{name:i18n.t('threadStates.notReported')})).toBeVisible();
  expect(screen.queryByRole('img',{name:/Running|Finished/})).not.toBeInTheDocument();
});

it('starts the tab icon subscription after the addressed conversation paints', async () => {
  const nodes = acmeTree();
  calls.pages.mockResolvedValue({ nodes, ancestors: [], hidden: [], seq: 10, since: null, complete: true, feed: hierarchyFeed() });
  let paint!: () => void;
  const afterThreadContent = new Promise<void>((resolve) => { paint = resolve; });
  vi.stubGlobal('__orchStartupReads', { afterThreadContent });
  try {
    show(<LiveThreadStateIcon threadId={ACME.mainThread} rootId={ACME.org} />);
    expect(calls.pages).not.toHaveBeenCalled();
    expect(listener).toBeUndefined();
    await act(async () => { paint(); await afterThreadContent; });
    await waitFor(() => expect(listener).toBeDefined());
    expect(calls.pages).toHaveBeenCalledTimes(1);
    expect(calls.pages.mock.calls[0]?.[1]).toEqual({ path_to: ACME.mainThread });
  } finally {
    vi.unstubAllGlobals();
  }
});
