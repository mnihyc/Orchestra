// @vitest-environment node
/** The live connection: who it says it is, what it does when the line goes, and how a feed stays equal to the server's rows whatever frames are lost. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { Approval, HarnessRequest, Item, ServerFrame, TreeNode } from '@/generated/api-types';
import { type ClientFrame, createRealtimeSocket, type SocketLike } from '@/realtime/socket';
import { treeWriter, useTreeStore } from '@/realtime/stores/tree';
import { threadWriter, threadViewOf, waitingInputs, type ThreadRows } from '@/realtime/stores/thread';
import { approvalItem, harnessApproval, threadInput, AT } from '../doubles/thread-rows';
import { me, userRef } from '../doubles/fixtures';
import { type DurableFeed, FeedRegistry, followFeed, type Loaded, UNSUBSCRIBE_AFTER_MS } from '@/realtime/sync';
import { beginOpening, openingId } from '@/lib/opening';

/** A socket that a test plays the server for. */
class FakeSocket implements SocketLike {
  readyState = 0;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: { code: number }) => void) | null = null;
  onerror: (() => void) | null = null;
  sent: ClientFrame[] = [];
  closedWith: number | undefined;

  send(data: string): void {
    this.sent.push(JSON.parse(data) as ClientFrame);
  }

  close(code?: number): void {
    this.closedWith = code;
  }

  /** The server accepts the connection. */
  accept(): void {
    this.readyState = 1;
    this.onopen?.();
  }

  /** The server says something. */
  say(frame: Record<string, unknown>): void {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }

  /** The line goes, with the code the server (or nobody) gave. */
  drop(code = 1006): void {
    this.readyState = 3;
    this.onclose?.({ code });
  }

  ready(): void {
    this.accept();
    this.say({ op: 'ready', connection_id: 'c1', user_id: 'u1', server_time: '', access_expires_at: '', heartbeat_secs: 25, build: 'b1' });
  }

  of<Op extends ClientFrame['op']>(op: Op): Extract<ClientFrame, { op: Op }>[] {
    return this.sent.filter((frame): frame is Extract<ClientFrame, { op: Op }> => frame.op === op);
  }
}

function harness(options: { token?: () => string | null; renew?: () => Promise<boolean> } = {}) {
  const sockets: FakeSocket[] = [];
  const suspended = vi.fn();
  const socket = createRealtimeSocket({
    url: () => 'ws://orch.test/api/v1/ws',
    token: options.token ?? (() => 'token-1'),
    renew: options.renew ?? (() => Promise.resolve(true)),
    onSuspended: suspended,
    connect: () => {
      const made = new FakeSocket();
      sockets.push(made);
      return made;
    },
    // No random part in the pauses, so that a test can say when the next attempt is.
    random: () => 0.5,
  });
  const last = (): FakeSocket => {
    const made = sockets.at(-1);
    if (made === undefined) throw new Error('no socket was made yet');
    return made;
  };
  return { socket, sockets, suspended, last };
}

const node = (id: string, seq: number, name = id): TreeNode => ({
  participating: true,
  id,
  address: null,
  kind: 'project',
  name,
  parent_id: 'org',
  color: null,
  docs: 0,
  archived: false,
  deleted: false,
  restricted: false,
  chat: { id: `chat-${id}`, seq: 0 },
  thread: null,
  topic: null,
  tree_seq: seq,
});

/** The server's rows of one tree, and the feed that follows them into the store. */
function tree(orgId: string, rows: TreeNode[], seq: number) {
  const server = { rows: new Map(rows.map((row) => [row.id, row])), seq };
  const loads: (number | null)[] = [];
  const feed: DurableFeed<TreeNode[]> = {
    feed: `org:${orgId}`,
    load: (since): Promise<Loaded<TreeNode[]>> => {
      loads.push(since);
      const data = [...server.rows.values()].filter((row) => since === null || row.tree_seq > since);
      return Promise.resolve({ seq: server.seq, complete: true, data });
    },
    reset: (nodes, at) => treeWriter.reset(orgId, nodes, at),
    apply: (nodes, at) => treeWriter.apply(orgId, nodes, at),
    read: (data) => data.nodes as TreeNode[],
  };
  /** Something changes on the server; the frame that would say so is returned, to be sent or lost. */
  const change = (row: TreeNode) => {
    const prev = server.seq;
    server.seq = row.tree_seq;
    server.rows.set(row.id, row);
    return { op: 'sync', feed: feed.feed, prev, seq: row.tree_seq, data: { nodes: [row] } };
  };
  const held = () => useTreeStore.getState().byOrg.get(orgId);
  const names = () => [...(held()?.nodes.values() ?? [])].map((row) => row.name).sort();
  return { server, feed, loads, change, held, names };
}

/** Lets what waits on answers run: the loads and repairs of a feed are promises. */
const settle = async () => {
  for (let turn = 0; turn < 10; turn += 1) await Promise.resolve();
};

beforeEach(() => {
  vi.useFakeTimers();
  treeWriter.clear();
  threadWriter.clear();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('the connection', () => {
  it('after second-step proof asks again only for refused feeds whose views still want them', () => {
    const { socket, last } = harness();
    socket.start(); last().ready();
    const listener = () => ({ since: () => null, onFrame: vi.fn() });
    socket.subscribe('org:held', listener());
    socket.subscribe('thread:private', listener());
    socket.subscribe('user', listener());
    socket.subscribe('org:closed', listener());
    const departed = socket.subscribe('org:departed', listener());
    for (const frame of last().of('sub')) {
      if (frame.feed === 'org:closed') last().say({ op: 'closed', id: frame.id, feed: frame.feed, reason: 'revoked' });
      else if (frame.feed !== 'user') last().say({ op: 'error', id: frame.id, code: frame.feed === 'thread:private' ? 'not_found' : 'forbidden', detail: 'this session cannot follow it' });
    }
    departed.unsubscribe();
    const before = last().of('sub').length;
    socket.retryRefused();
    expect(last().of('sub').slice(before).map((frame) => frame.feed)).toEqual(['org:held', 'thread:private']);
    expect(last().of('sub').slice(before).every((frame) => frame.id > before)).toBe(true);
    const sent = last().of('sub').length;
    socket.retryRefused();
    expect(last().of('sub')).toHaveLength(sent);
    socket.stop();
  });

  it('dispatches a generated subscription frame and keeps a connection-wide error out of that feed', () => {
    const { socket, last } = harness();
    const receive = vi.fn();
    socket.start();
    last().ready();
    socket.subscribe('thread:a:raw', { since: () => null, onFrame: receive });
    const id = last().of('sub')[0]?.id;
    if (id === undefined) throw new Error('the subscription was not sent');
    last().say({ op: 'error', code: 'forbidden', detail: 'The connection cannot do that.' } satisfies ServerFrame);
    expect(receive).not.toHaveBeenCalled();
    const frame = { op: 'subscribed', id, feed: 'thread:a:raw', seq: null, mode: 'ephemeral' } satisfies ServerFrame;
    last().say(frame);
    expect(receive).toHaveBeenCalledWith(frame);
    last().say({ op: 'event', id, feed: 'thread:a:raw', data: { n: 1, payload: { line: 'hello' } } } satisfies ServerFrame);
    expect(receive).toHaveBeenLastCalledWith(expect.objectContaining({ data: { n: 1, payload: { line: 'hello' } } }));
    socket.stop();
  });
  it('keeps a view’s opening nonce on reconnect while a newly opened feed uses the new navigation nonce', () => {
    const { socket, last } = harness();
    beginOpening();
    const first = openingId();
    socket.start(); last().ready();
    socket.subscribe('thread:private', { since: () => 1, onFrame: vi.fn() });
    expect(last().of('sub')[0]?.opening_id).toBe(first);
    beginOpening();
    const next = openingId();
    last().drop(); vi.advanceTimersByTime(500); last().ready();
    expect(last().of('sub')[0]?.opening_id).toBe(first);
    socket.subscribe('document:next', { since: () => 2, onFrame: vi.fn() });
    expect(last().of('sub').at(-1)?.opening_id).toBe(next);
    expect(next).not.toBe(first);
    socket.stop();
  });
  it('says who it is with its first frame, and is ready when the server says so', () => {
    const { socket, last } = harness();
    socket.start();
    expect(socket.status.getState().phase).toBe('connecting');

    last().accept();
    expect(last().sent).toEqual([{ op: 'auth', token: 'token-1' }]);
    expect(socket.status.getState().phase).toBe('authenticating');

    last().say({ op: 'ready', connection_id: 'c1', user_id: 'u1', server_time: '', access_expires_at: '', heartbeat_secs: 25, build: 'b7' });
    expect(socket.status.getState()).toMatchObject({ phase: 'ready', lostAt: null, build: 'b7' });
  });

  it('does not connect while nobody is signed in', () => {
    const { socket, sockets } = harness({ token: () => null });
    socket.start();

    expect(sockets).toHaveLength(0);
    expect(socket.status.getState().phase).toBe('idle');
  });

  it('says who it is again when its token was renewed', () => {
    let token = 'token-1';
    const { socket, last } = harness({ token: () => token });
    socket.start();
    last().ready();

    token = 'token-2';
    socket.reauth();

    expect(last().of('reauth')).toEqual([{ op: 'reauth', token: 'token-2' }]);
  });

  it('comes back by itself with pauses that double, up to fifteen seconds', () => {
    const { socket, sockets, last } = harness();
    socket.start();
    last().ready();

    const pauses: number[] = [];
    for (let attempt = 0; attempt < 7; attempt += 1) {
      last().drop();
      expect(socket.status.getState().phase).toBe('waiting');
      const before = sockets.length;
      let waited = 0;
      while (sockets.length === before) {
        vi.advanceTimersByTime(250);
        waited += 250;
      }
      pauses.push(waited);
    }

    expect(pauses).toEqual([500, 1_000, 2_000, 4_000, 8_000, 15_000, 15_000]);
  });

  it('begins with a short pause again after a connection that held', () => {
    const { socket, sockets, last } = harness();
    socket.start();
    last().ready();
    last().drop();
    vi.advanceTimersByTime(500);
    last().drop();
    // The second pause is a second long.
    vi.advanceTimersByTime(999);
    expect(sockets).toHaveLength(2);
    vi.advanceTimersByTime(1);
    expect(sockets).toHaveLength(3);

    // This one holds for half a minute, pings answered, and then goes.
    last().ready();
    for (let beat = 1; beat <= 2; beat += 1) {
      vi.advanceTimersByTime(25_000);
      last().say({ op: 'pong', n: last().of('ping').at(-1)?.n });
    }
    last().drop();
    vi.advanceTimersByTime(500);

    expect(sockets).toHaveLength(4);
  });

  it('remembers since when the line is gone, for the notice that says so after a moment', () => {
    const { socket, last } = harness();
    socket.start();
    last().ready();
    vi.setSystemTime(new Date('2026-10-11T12:00:00Z'));

    last().drop();
    const lostAt = socket.status.getState().lostAt;
    expect(lostAt).toBe(Date.parse('2026-10-11T12:00:00Z'));

    // Another failed attempt does not move the moment.
    vi.advanceTimersByTime(500);
    last().drop();
    expect(socket.status.getState().lostAt).toBe(lostAt);

    vi.advanceTimersByTime(1_000);
    last().ready();
    expect(socket.status.getState().lostAt).toBeNull();
  });

  it('asks for a new token first when the server says the old one is no longer good', async () => {
    const renew = vi.fn(() => Promise.resolve(true));
    const { socket, sockets, last } = harness({ renew });
    socket.start();
    last().ready();

    last().drop(4401);
    await settle();

    expect(renew).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(500);
    expect(sockets).toHaveLength(2);
  });

  it('gives up, and says so, when the server speaks another version of the protocol', () => {
    const { socket, sockets, last } = harness();
    socket.start();
    last().accept();

    last().drop(4409);
    vi.advanceTimersByTime(60_000);

    expect(sockets).toHaveLength(1);
    expect(socket.status.getState()).toMatchObject({ phase: 'idle', outdated: true });
  });

  it('tells the page when the server says its person is suspended, and stays away', () => {
    const { socket, sockets, suspended, last } = harness();
    socket.start();
    last().ready();

    last().drop(4403);
    vi.advanceTimersByTime(60_000);

    expect(suspended).toHaveBeenCalledTimes(1);
    expect(sockets).toHaveLength(1);
  });

  it('measures the round trip with every heartbeat and takes the middle one of the last three', () => {
    const { socket, last } = harness();
    vi.setSystemTime(0);
    socket.start();
    last().accept();
    vi.advanceTimersByTime(40);
    last().say({ op: 'ready', connection_id: 'c', user_id: 'u', server_time: '', access_expires_at: '', heartbeat_secs: 25, build: 'b' });
    expect(socket.status.getState().roundTripMs).toBe(40);

    // Two more: one quick, one that a late packet made slow.
    for (const late of [20, 900]) {
      vi.advanceTimersByTime(25_000);
      const ping = last().of('ping').at(-1);
      vi.advanceTimersByTime(late);
      last().say({ op: 'pong', n: ping?.n });
    }

    // 40, 20 and 900: one late packet changes nothing.
    expect(socket.status.getState().roundTripMs).toBe(40);
  });

  it('counts a connection that says nothing for a minute as gone, and connects again', () => {
    const { socket, sockets, last } = harness();
    socket.start();
    last().ready();

    // Pings go out; nothing comes back.
    vi.advanceTimersByTime(75_000);
    expect(socket.status.getState().phase).toBe('waiting');
    vi.advanceTimersByTime(500);

    expect(sockets).toHaveLength(2);
  });

  it('connects at once when the browser says the line is back', () => {
    const { socket, sockets, last } = harness();
    socket.start();
    last().ready();
    for (let attempt = 0; attempt < 5; attempt += 1) {
      last().drop();
      vi.advanceTimersByTime(20_000);
    }
    last().drop();
    const before = sockets.length;

    socket.wake();

    expect(sockets).toHaveLength(before + 1);
  });
});

describe('a feed that is kept in order', () => {
  it.each([false, true])('retires an old authority load without waiting for its transport (incremental=%s)', async (incremental) => {
    const { socket, last } = harness(); socket.start(); last().ready();
    interface Data { words: string[] }
    const pending: { since: number | null; signal: AbortSignal; resolve: (reply: Loaded<Data>) => void; reject: (error: Error) => void }[] = [];
    let visible: string[] = [];
    const failed = vi.fn();
    const loads = vi.fn((since: number | null, signal: AbortSignal) => new Promise<Loaded<Data>>((resolve, reject) => pending.push({ since, signal, resolve, reject })));
    const feed: DurableFeed<Data> = { feed: 'doc:authority-probe', load: loads, read: (rows) => {
      if (!Array.isArray(rows.words) || !rows.words.every((word: unknown) => typeof word === 'string')) throw new Error('invalid authority probe');
      return { words: rows.words.filter((word: unknown): word is string => typeof word === 'string') };
    },
      reset: (data) => { visible = data.words; }, apply: (data) => { visible = data.words; }, failed };
    const registry = new FeedRegistry();
    const follower = followFeed(socket, feed);
    registry.use(feed.feed, () => follower, () => { visible = []; });
    await settle();
    if (incremental) {
      pending[0]!.resolve({ seq: 1, complete: true, data: { words: ['former authority'] } }); await settle();
      follower.refresh(); await settle();
    }
    const former = pending.at(-1)!;
    registry.rebase(); await settle();
    const ordinary = pending.at(-1)!;
    expect(ordinary).not.toBe(former);
    expect(ordinary.since).toBeNull();
    expect(former.signal.aborted).toBe(true);
    expect(visible).toEqual([]);
    // The old transport ignores abort and answers after eviction. It must publish nothing.
    former.resolve({ seq: 99, complete: true, data: { words: ['private old answer'] } }); await settle();
    expect(visible).toEqual([]);
    expect(follower.lastSeq()).toBeNull();
    ordinary.reject(new Error('ordinary reader refused')); await settle();
    expect(visible).toEqual([]);
    expect(failed).toHaveBeenCalledTimes(1);
    follower.reload(); await settle();
    pending.at(-1)!.resolve({ seq: 2, complete: true, data: { words: ['ordinary current answer'] } }); await settle();
    expect(visible).toEqual(['ordinary current answer']);
    expect(follower.lastSeq()).toBe(2);
    registry.clear();
  });

  it.each([false,true])('keeps an open request live through a capacity refusal and its decision (question=%s)',async(question)=>{
    const {socket,last}=harness();socket.start();last().ready();
    const other=socket.subscribe('org:other',{since:()=>0,onFrame:()=>undefined});
    const id='watched-thread';const loads=vi.fn(()=>Promise.resolve({seq:1,complete:true,data:{items:[]}}));
    const feed:DurableFeed<ThreadRows>={feed:`thread:${id}`,load:loads,
      reset:(rows)=>threadWriter.reset(id,rows,false),apply:(rows)=>threadWriter.apply(id,rows),read:(rows)=>rows};
    const follower=followFeed(socket,feed);await settle();
    const sub=last().of('sub').find((frame)=>frame.feed===feed.feed)?.id;
    last().say({op:'error',id:sub,code:'too_many_subscriptions',detail:'capacity'});other.unsubscribe();
    const retry=last().of('sub').at(-1)?.id;
    expect(last().of('sub').at(-1)).toMatchObject({feed:feed.feed,since:1});
    const ada=userRef(me(1,'ada','Ada'));
    const request:HarnessRequest=question
      ? {request_kind:'user_input',options:[],detail:{questions:[{id:'scope',header:'Scope',question:'Which scope?',multiple:false,options:null,allow_other:true}]}}
      : {request_kind:'command',options:[{id:'allow_once',label:'Allow once',style:'allow',scope:'once',rule:null}],detail:{command:'./deploy staging',cwd:'/workspace',reason:null}};
    const approval=harnessApproval('approval',{threadId:id,itemId:'card',title:question?'Choose the scope':'Allow deployment',request,requester:ada});
    const card=approvalItem('card',approval.id);
    if(card.kind!=='approval')throw new Error('the request fixture must be an approval item');
    const pending:Item={...card,seq_created:2,seq_updated:2,payload:{...card.payload,approval}};
    last().say({op:'subscribed',id:retry,feed:feed.feed,seq:1,mode:'delta'});
    last().say({op:'sync',id:retry,feed:feed.feed,prev:1,seq:2,data:{items:[pending]}});await settle();
    expect(threadViewOf(id).items.get('card')).toEqual(pending);
    const answered:Approval={...approval,state:'approved',decided_at:AT,decided_by:ada,
      decision:question?{answers:{scope:['Login']},note:null}:{option_id:'allow_once',note:null}};
    const decided:Item={...pending,status:'approved',seq_updated:3,payload:{...card.payload,approval:answered}};
    last().say({op:'sync',id:retry,feed:feed.feed,prev:2,seq:3,data:{items:[decided]}});await settle();
    expect(threadViewOf(id).items.get('card')).toEqual(decided);
    expect(loads).toHaveBeenCalledTimes(1);expect(follower.lastSeq()).toBe(3);follower.stop();
  });

  it('removes a delivered input from the visible queue after a refused feed obtains a released slot',async()=>{
    const {socket,last}=harness();socket.start();last().ready();
    const other=socket.subscribe('org:other',{since:()=>0,onFrame:()=>undefined});
    const id='delivery-thread';const pending={...threadInput('input','Delivered words',{author:null,position:'1',state:'delivering'}),seq_updated:1};
    const loads=vi.fn(()=>Promise.resolve({seq:1,complete:true,data:{inputs:[pending]}}));
    const follower=followFeed<ThreadRows>(socket,{feed:`thread:${id}`,load:loads,read:(rows)=>rows,
      reset:(rows)=>threadWriter.reset(id,rows,false),apply:(rows)=>threadWriter.apply(id,rows)});await settle();
    expect(waitingInputs(threadViewOf(id))).toEqual([pending]);
    const sub=last().of('sub').find((frame)=>frame.feed===`thread:${id}`)?.id;
    last().say({op:'error',id:sub,code:'too_many_subscriptions',detail:'capacity'});other.unsubscribe();
    const delivered={...pending,state:'delivered',delivered_at:AT,seq_updated:2};
    last().say({op:'sync',id:last().of('sub').at(-1)?.id,feed:`thread:${id}`,prev:1,seq:2,data:{inputs:[delivered]}});await settle();
    expect(threadViewOf(id).inputs.get('input')).toEqual(delivered);
    expect(waitingInputs(threadViewOf(id))).toEqual([]);expect(loads).toHaveBeenCalledTimes(1);follower.stop();
  });

  it('keeps a capacity-refused tree wanted and applies renames and new threads when another view releases a slot',async()=>{
    const {socket,last}=harness();socket.start();last().ready();
    const other=socket.subscribe('org:other',{since:()=>0,onFrame:()=>undefined});
    const acme=tree('acme',[node('thread',1,'Original name')],1);
    const follower=followFeed(socket,acme.feed);await settle();
    const sub=last().of('sub').find((frame)=>frame.feed==='org:acme');
    if(sub===undefined)throw new Error('the visible tree must subscribe');
    last().say({op:'error',id:sub.id,code:'too_many_subscriptions',detail:'a connection follows at most 64 feeds'});
    const sent=last().of('sub').length;
    vi.advanceTimersByTime(60_000);await settle();
    expect(last().of('sub')).toHaveLength(sent);
    other.unsubscribe();
    expect(last().of('sub').at(-1)).toMatchObject({feed:'org:acme',since:1});
    const retried=last().of('sub').at(-1)?.id;
    last().say({op:'subscribed',id:retried,feed:'org:acme',seq:1,mode:'delta'});
    last().say({...acme.change(node('thread',2,'Renamed thread')),id:retried});
    last().say({...acme.change(node('new-thread',3,'New thread')),id:retried});
    await settle();
    expect(acme.names()).toEqual(['New thread','Renamed thread']);
    expect(follower.lastSeq()).toBe(3);
    expect(acme.loads).toEqual([null]);
    follower.stop();
  });

  it('cancels a capacity-refused hidden feed before any other view releases a slot',async()=>{
    const {socket,last}=harness();socket.start();last().ready();
    const other=socket.subscribe('org:other',{since:()=>0,onFrame:()=>undefined});
    const acme=tree('acme',[node('thread',1)],1);
    const follower=followFeed(socket,acme.feed);await settle();
    const id=last().of('sub').find((frame)=>frame.feed==='org:acme')?.id;
    last().say({op:'error',id,code:'too_many_subscriptions',detail:'capacity'});
    follower.stop();const sent=last().of('sub').length;
    other.unsubscribe();
    expect(last().of('sub')).toHaveLength(sent);
    last().drop();vi.advanceTimersByTime(500);last().ready();
    expect(last().of('sub')).toEqual([]);
  });

  it('is loaded over HTTP first and subscribed from the sequence of that answer', async () => {
    const { socket, last } = harness();
    socket.start();
    last().ready();
    const acme = tree('acme', [node('a', 1), node('b', 2)], 2);

    followFeed(socket, acme.feed);
    await settle();

    expect(acme.loads).toEqual([null]);
    expect(last().of('sub')).toEqual([{ op: 'sub', id: 1, feed: 'org:acme', since: 2, opening_id: openingId() }]);
    expect(acme.names()).toEqual(['a', 'b']);
  });

  it('takes the rows of each frame that follows the one before', async () => {
    const { socket, last } = harness();
    socket.start();
    last().ready();
    const acme = tree('acme', [node('a', 1)], 1);
    followFeed(socket, acme.feed);
    await settle();
    const sub = last().of('sub')[0]?.id;

    last().say({ ...acme.change(node('b', 2)), id: sub });
    last().say({ ...acme.change(node('a', 3, 'a renamed')), id: sub });
    await settle();

    expect(acme.names()).toEqual(['a renamed', 'b']);
    expect(acme.held()?.lastSeq).toBe(3);
    // Nothing was lost, so nothing was asked of HTTP again.
    expect(acme.loads).toEqual([null]);
  });

  it('repairs a gap: a frame that went missing is asked for over HTTP before the next one is taken', async () => {
    const { socket, last } = harness();
    socket.start();
    last().ready();
    const acme = tree('acme', [node('a', 1)], 1);
    followFeed(socket, acme.feed);
    await settle();
    const sub = last().of('sub')[0]?.id;

    // The frame that brings `b` is lost on the way; the one that brings `c` arrives.
    acme.change(node('b', 2));
    last().say({ ...acme.change(node('c', 3)), id: sub });
    await settle();

    expect(acme.loads).toEqual([null, 1]);
    expect(acme.names()).toEqual(['a', 'b', 'c']);
    expect(acme.held()?.lastSeq).toBe(3);
    // The store holds what the server holds.
    expect([...(acme.held()?.nodes.values() ?? [])]).toEqual([...acme.server.rows.values()]);
  });

  it('drops a frame that came twice', async () => {
    const { socket, last } = harness();
    socket.start();
    last().ready();
    const acme = tree('acme', [node('a', 1)], 1);
    const applied = vi.spyOn(acme.feed, 'apply');
    followFeed(socket, acme.feed);
    await settle();
    const sub = last().of('sub')[0]?.id;
    const frame = { ...acme.change(node('b', 2)), id: sub };

    last().say(frame);
    last().say(frame);
    await settle();

    expect(applied).toHaveBeenCalledTimes(1);
    expect(acme.loads).toEqual([null]);
    expect(acme.names()).toEqual(['a', 'b']);
  });

  it('never puts an older row in the place of a newer one', async () => {
    const { socket, last } = harness();
    socket.start();
    last().ready();
    const acme = tree('acme', [node('a', 5, 'a as it is now')], 5);
    followFeed(socket, acme.feed);
    await settle();
    const sub = last().of('sub')[0]?.id;

    // A frame that overlaps what is held: its sequence is new, one of its rows is not.
    last().say({ op: 'sync', id: sub, feed: 'org:acme', prev: 5, seq: 6, data: { nodes: [node('a', 4, 'a as it was'), node('b', 6)] } });
    await settle();

    expect(acme.names()).toEqual(['a as it is now', 'b']);
  });

  it('subscribes again after a reconnect, from where it had come to, and takes what changed meanwhile', async () => {
    const { socket, sockets, last } = harness();
    socket.start();
    last().ready();
    const acme = tree('acme', [node('a', 1)], 1);
    followFeed(socket, acme.feed);
    await settle();
    last().say({ ...acme.change(node('b', 2)), id: last().of('sub')[0]?.id });
    await settle();

    last().drop();
    // While the line is gone the server goes on.
    acme.change(node('c', 3));
    vi.advanceTimersByTime(500);
    expect(sockets).toHaveLength(2);
    last().ready();

    const again = last().of('sub');
    expect(again).toEqual([{ op: 'sub', id: again[0]?.id, feed: 'org:acme', since: 2, opening_id: openingId() }]);

    // The server answers with what changed since.
    last().say({ op: 'subscribed', id: again[0]?.id, feed: 'org:acme', seq: 3, mode: 'delta' });
    last().say({ op: 'sync', id: again[0]?.id, feed: 'org:acme', prev: 2, seq: 3, data: { nodes: [node('c', 3)] } });
    await settle();

    expect(acme.names()).toEqual(['a', 'b', 'c']);
    // The reconnect cost no request over HTTP: the feed itself brought what was missing.
    expect(acme.loads).toEqual([null]);
  });

  it('loads everything again when the server says what it holds cannot be brought up to date', async () => {
    const { socket, last } = harness();
    socket.start();
    last().ready();
    const acme = tree('acme', [node('a', 1), node('gone', 2)], 2);
    followFeed(socket, acme.feed);
    await settle();
    const first = last().of('sub')[0]?.id;

    // The server's rows were put back to an earlier state: what the page holds is from a future that is gone.
    acme.server.rows.delete('gone');
    acme.server.seq = 1;
    last().say({ op: 'subscribed', id: first, feed: 'org:acme', seq: 1, mode: 'reload' });
    await settle();

    expect(acme.loads).toEqual([null, null]);
    expect(acme.names()).toEqual(['a']);
    expect(last().of('sub').at(-1)).toMatchObject({ feed: 'org:acme', since: 1 });
  });

  it('replaces an old authority projection and rejoins when the server requires a full reload at the same sequence', async () => {
    const { socket, last } = harness(); socket.start(); last().ready();
    const acme = tree('acme', [node('visible', 1), node('formerly-private', 1)], 1);
    const invalidated = vi.fn();
    const feed = { ...acme.feed, invalidated, resetting: () => treeWriter.evict('acme') };
    followFeed(socket, feed); await settle();
    const original = last().of('sub')[0]?.id;
    acme.server.rows.delete('formerly-private');
    last().say({ op: 'resync', id: original, feed: 'org:acme', reload: true });
    await settle();
    expect(invalidated).toHaveBeenCalledOnce();
    expect(acme.loads).toEqual([null, null]);
    expect(acme.names()).toEqual(['visible']);
    const next = last().of('sub').at(-1);
    expect(next?.id).not.toBe(original);
    expect(next).toMatchObject({ feed: 'org:acme', since: 1 });
    last().say({ op: 'sync', id: original, feed: 'org:acme', prev: 1, seq: 2, data: { nodes: [node('late-private', 2)] } });
    await settle();
    expect(acme.names()).toEqual(['visible']);
  });

  it.each(['resync', 'subscribed'] as const)('retires pending authority work immediately on a native %s reload', async (op) => {
    const { socket, last } = harness(); socket.start(); last().ready();
    const pending: { since: number | null; signal: AbortSignal; resolve: (value: Loaded<string[]>) => void }[] = [];
    let visible: string[] = [];
    const invalidated = vi.fn();
    const follower = followFeed(socket, {
      feed: 'org:retiring', invalidated, resetting: () => { visible = []; },
      load: (since, signal) => new Promise<Loaded<string[]>>((resolve) => pending.push({ since, signal, resolve })),
      reset: (rows) => { visible = rows; }, apply: (rows) => { visible = rows; }, read: () => [],
    });
    await settle();
    pending[0]!.resolve({ seq: 1, complete: true, data: ['former private row'] }); await settle();
    const oldSubscription = last().of('sub').at(-1)!.id;
    follower.refresh(); await settle();
    const oldRead = pending[1]!;
    last().say(op === 'resync'
      ? { op, id: oldSubscription, feed: 'org:retiring', reload: true }
      : { op, id: oldSubscription, feed: 'org:retiring', seq: 1, mode: 'reload' });
    expect(visible).toEqual([]);
    expect(oldRead.signal.aborted).toBe(true);
    await settle();
    expect(pending.map((read) => read.since)).toEqual([null, 1, null]);
    oldRead.resolve({ seq: 9, complete: true, data: ['late former private row'] }); await settle();
    expect(visible).toEqual([]);
    pending[2]!.resolve({ seq: 1, complete: true, data: ['current row'] }); await settle();
    expect(visible).toEqual(['current row']);
    expect(last().of('sub').at(-1)!.id).not.toBe(oldSubscription);
    expect(invalidated).toHaveBeenCalledTimes(op === 'resync' ? 1 : 0);
    follower.stop();
  });

  it('asks HTTP for what changed when the server says frames were dropped on the way', async () => {
    const { socket, last } = harness();
    socket.start();
    last().ready();
    const acme = tree('acme', [node('a', 1)], 1);
    followFeed(socket, acme.feed);
    await settle();

    acme.change(node('b', 2));
    last().say({ op: 'resync', id: last().of('sub')[0]?.id, feed: 'org:acme' });
    await settle();

    expect(acme.loads).toEqual([null, 1]);
    expect(acme.names()).toEqual(['a', 'b']);
  });

  it('is not subscribed again once the server closed it: the reader may no longer see the thing', async () => {
    const { socket, last } = harness();
    socket.start();
    last().ready();
    const acme = tree('acme', [node('a', 1)], 1);
    const closed = vi.fn();
    followFeed(socket, { ...acme.feed, closed });
    await settle();

    last().say({ op: 'closed', id: last().of('sub')[0]?.id, feed: 'org:acme', reason: 'revoked' });
    last().drop();
    vi.advanceTimersByTime(500);
    last().ready();

    expect(closed).toHaveBeenCalledWith('revoked');
    expect(last().of('sub')).toEqual([]);
  });
});

describe('a feed that several views show', () => {
  it('a refused visible tree returns as a whole current tree after second-step proof and then follows new rows', async () => {
    const { socket, last } = harness();
    socket.start(); last().ready();
    const acme = tree('acme', [node('a', 1)], 1);
    const follower = followFeed(socket, { ...acme.feed, closed: () => treeWriter.evict('acme') });
    await settle();
    last().say({ op: 'error', id: last().of('sub').at(-1)?.id, code: 'forbidden', detail: 'you may not follow that feed' });
    expect(acme.names()).toEqual([]);
    acme.change(node('b', 2));
    socket.retryRefused();
    const asked = last().of('sub').at(-1);
    if (asked === undefined) throw new Error('the still-visible tree was retried');
    expect(asked.since).toBeUndefined();
    last().say({ op: 'subscribed', id: asked.id, feed: asked.feed, seq: null, mode: 'reload' } satisfies ServerFrame);
    await settle();
    expect(acme.names()).toEqual(['a', 'b']);
    expect(acme.loads).toEqual([null, null]);
    const fresh = last().of('sub').at(-1);
    last().say({ ...acme.change(node('c', 3)), id: fresh?.id });
    await settle();
    expect(acme.names()).toEqual(['a', 'b', 'c']);
    follower.stop(); socket.stop();
  });

  it('is followed once, and for ten seconds after the last view left', async () => {
    const { socket, last } = harness();
    socket.start();
    last().ready();
    const acme = tree('acme', [node('a', 1)], 1);
    const registry = new FeedRegistry();
    const follow = () => followFeed(socket, acme.feed);
    const evict = vi.fn();

    const first = registry.use('org:acme', follow, evict);
    const second = registry.use('org:acme', follow, evict);
    await settle();
    expect(last().of('sub')).toHaveLength(1);

    first();
    second();
    vi.advanceTimersByTime(UNSUBSCRIBE_AFTER_MS - 1);
    expect(last().of('unsub')).toHaveLength(0);

    // Somebody comes back within the ten seconds: nothing was given up, nothing is asked again.
    const third = registry.use('org:acme', follow, evict);
    vi.advanceTimersByTime(UNSUBSCRIBE_AFTER_MS);
    expect(last().of('unsub')).toHaveLength(0);
    expect(acme.loads).toEqual([null]);

    third();
    vi.advanceTimersByTime(UNSUBSCRIBE_AFTER_MS);
    expect(last().of('unsub')).toHaveLength(1);
    expect(evict).not.toHaveBeenCalled();

    // What it delivered is kept a while longer, and then forgotten.
    vi.advanceTimersByTime(5 * 60_000);
    expect(evict).toHaveBeenCalledTimes(1);
  });
});
