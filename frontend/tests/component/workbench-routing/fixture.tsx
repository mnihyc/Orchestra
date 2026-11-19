/** Synthetic feature bodies expose their real PageView context; no router hook/context is mocked. */
import { useEffect, useLayoutEffect, useState, type ReactNode } from 'react';
import { client } from '@/api/client';
import type { Node, Thread } from '@/generated/api-types';
import { usePageContext, type PageContextValue, type PageNavigation } from '@/features/workbench/pageContext';
import type { ThreadProps } from '@/features/thread/Thread';
import type { OrgHomeProps } from '@/features/org/Home';
import type { OrgSettingsProps } from '@/features/settings/org/OrgSettings';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { threadViewOf, threadWriter, useThreadView } from '@/realtime/stores/thread';
import type { ThreadReadTarget } from '@/realtime/threadScope';
import { ACME, me, org, userRef } from '../../doubles/fixtures';
import { World } from '../../doubles/world';

export const USER = 'workbench-routing-reader';
export const ORG_ID = '00000000-0000-4000-8000-000000000091';
export const ORG_ADDRESS = 'synthetic.research';
export const department = org(ORG_ID, 'research', 'Synthetic Research', userRef(me(91, 'reader', 'Reader')), { address: ORG_ADDRESS });

let baseThread: Thread | undefined;
function syntheticThread(id: string): Thread {
  if (baseThread === undefined) {
    const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
    if (person === undefined || row === undefined) throw new Error('The synthetic native thread template is missing.');
    baseThread = world.threads.thread(person, row);
  }
  const name = `Thread ${id}`;
  return { ...baseThread, id, name, path: baseThread.path.map((part) => part.kind === 'thread' ? { ...part, id, name } : part) };
}
export const metadataDemands: { owner: string; threadId: string; reportVisible: boolean }[] = [];
/** Native data seam only: real TargetContext, selection/read providers, cache and heading reporters execute. */
export function useSyntheticThreadFeed(target: ThreadReadTarget, reportVisible = true): void {
  const read = useThreadReadRef(target);
  const selection = useThreadView(read).read?.selection ?? null;
  useLayoutEffect(() => {
    metadataDemands.push({ owner: read.key.owner, threadId: read.threadId, reportVisible });
    const current = threadViewOf(read);
    if (current.thread?.id === read.threadId && current.read?.ready === true) return;
    const captured = threadWriter.capture(read, client.session.getState().readerVersion);
    threadWriter.reset(read, { thread: syntheticThread(read.threadId), items: [], turns: [], segments: [], projection: {
      viewed_continuation_id: captured.scope.selection ?? '00000000-0000-7000-8000-000000000092',
      active_continuation_id: '00000000-0000-7000-8000-000000000092',
      follows_active: captured.scope.selection === null, projection_revision: 1, execution_revision: 1,
    } }, false, captured);
  }, [read, selection, reportVisible]);
}

export interface Deferred<Value> {
  promise: Promise<Value>;
  resolve: (value: Value) => void;
  reject: (error: Error) => void;
  settled: () => boolean;
}
function deferred<Value>(): Deferred<Value> {
  let accept: ((value: Value) => void) | undefined;
  let refuse: ((error: Error) => void) | undefined;
  let complete = false;
  const promise = new Promise<Value>((resolve, reject) => { accept = resolve; refuse = reject; });
  return {
    promise,
    resolve: (value) => {
      if (complete) return;
      complete = true;
      if (accept === undefined) throw new Error('The synthetic request did not install its resolver.');
      accept(value);
    },
    reject: (error) => {
      if (complete) return;
      complete = true;
      if (refuse === undefined) throw new Error('The synthetic request did not install its refusal.');
      refuse(error);
    },
    settled: () => complete,
  };
}

const canonical = new Map<string, Deferred<Node>>();
const pending = new Set<Deferred<Node>>();
export const canonicalCalls: string[] = [];
export function holdCanonical(id = ORG_ID): Deferred<Node> {
  const gate = deferred<Node>();
  canonical.set(id, gate);
  pending.add(gate);
  return gate;
}
/** Replaces only nodeQuery's queryFn. The native query key/client/resolver/route loaders remain real. */
export function readCanonicalNode(id: string): Promise<Node> {
  canonicalCalls.push(id);
  const gate = canonical.get(id);
  return gate === undefined ? Promise.reject(new Error(`Unplanned synthetic node read: ${id}`)) : gate.promise;
}
export function releasePendingCanonical(): void {
  for (const gate of pending) if (!gate.settled()) gate.resolve(department);
  pending.clear();
  canonical.clear();
}

export interface ObservedBody { owner: string; kind: string; pathname: string; search: Record<string, unknown>; token: number }
export const bodies: ObservedBody[] = [];
export const captures = new Map<string, PageContextValue>();
let nextToken = 0;
function useOwnPage(): PageContextValue {
  const page = usePageContext();
  if (page === null) throw new Error('The real PageView must provide this feature body.');
  return page;
}
export function capturedNavigate(owner: string, options: PageNavigation): Promise<void> {
  const capture = captures.get(owner);
  if (capture === undefined) throw new Error(`No mounted feature callback for ${owner}`);
  return capture.navigate(options);
}

function FeaturePane({ kind, children, readOwner, continuation }: { kind: string; children?: ReactNode; readOwner?: string; continuation?: string | null }) {
  const page = useOwnPage();
  const [token] = useState(() => { nextToken += 1; return nextToken; });
  const [local, setLocal] = useState('');
  useLayoutEffect(() => {
    captures.set(page.id, page);
    return () => { if (captures.get(page.id) === page) captures.delete(page.id); };
  }, [page]);
  useEffect(() => {
    bodies.push({ owner: page.id, kind, pathname: page.target.pathname, search: page.target.search, token });
  }, [page, kind, token]);
  return <section data-testid={`body-${page.id}`} data-kind={kind} data-pathname={page.target.pathname}
    data-search={JSON.stringify(page.target.search)} data-instance={token} data-read-owner={readOwner}
    data-continuation={continuation === undefined ? undefined : continuation ?? 'active'}>
    <input aria-label="Local view state" value={local} onChange={(event) => setLocal(event.currentTarget.value)} />
    <output aria-label="Own pathname">{page.target.pathname}</output>
    <output aria-label="Own search">{JSON.stringify(page.target.search)}</output>
    {children}
  </section>;
}

export function ThreadBody({ threadId, continuation, onVersionSelect }: ThreadProps) {
  const read = useThreadReadRef(threadId);
  return <FeaturePane kind="thread" readOwner={read.key.owner} continuation={continuation}>
    <button onClick={() => {
      if (onVersionSelect === undefined) throw new Error('The real thread route must supply version selection.');
      onVersionSelect('version-z');
    }}>Choose version Z</button>
  </FeaturePane>;
}
export function OrgBody({ slug, tab }: OrgHomeProps) {
  return <FeaturePane kind="org"><output aria-label="Org address">{slug}</output><output aria-label="Org tab">{tab}</output></FeaturePane>;
}
export function OrgSettingsBody({ slug, section }: OrgSettingsProps) {
  return <FeaturePane kind="org-settings"><output aria-label="Org address">{slug}</output><output aria-label="Settings section">{section}</output></FeaturePane>;
}
export function ManagementBody({ section }: { section: string }) {
  return <FeaturePane kind="management"><output aria-label="Management section">{section}</output></FeaturePane>;
}

export function BottomInspector() {
  return <button>Inspector focus target</button>;
}
export function resetFeatureEvidence(): void {
  bodies.length = 0;
  canonicalCalls.length = 0;
  captures.clear();
  metadataDemands.length = 0;
  nextToken = 0;
}
