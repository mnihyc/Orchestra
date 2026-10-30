/** Typed native-reader replies and feature-body witnesses; no harness work is simulated. */
import { useLayoutEffect, useState } from 'react';
import { client } from '@/api/client';
import type { Thread, Environment, Node } from '@/generated/api-types';
import type { ThreadProps } from '@/features/thread/Thread';
import type { DocumentPageProps } from '@/features/documents/Document';
import { usePageContext } from '@/features/workbench/pageContext';
import { useViewInstance } from '@/features/workbench/instance';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { threadViewOf, threadWriter, useThreadView } from '@/realtime/stores/thread';
import { type ThreadReadTarget } from '@/realtime/threadScope';
import { ACME } from '../../doubles/fixtures';
import { World } from '../../doubles/world';
import { detailEnvironment } from '../resource-detail-fixtures';

export const USER = 'content-workbench-reader';
export const nativeRows = new Map<string, Thread>();
export const nativeNodes = new Map<string, Node>();
export const heldFeeds = new Set<string>();
export const delayedReads = new Map<string, () => void>();
export const feedCalls: { native: string; owner: string; controls: boolean; visible: boolean }[] = [];
let template: Thread | undefined;
export function nativeThread(id: string, env = `environment-${id}`): Thread {
  if (template === undefined) {
    const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
    if (person === undefined || row === undefined) throw new Error('The synthetic native reader fixture is missing.');
    template = world.threads.thread(person, row);
  }
  return { ...template, id, name: `Named ${id}`, environment: { id: env, name: env, address: `synthetic.${env}`, owner: template.owner, placement: 'managed', status: 'running', ports_exposed: false },
    environment_permissions: ['files.view', 'terminal.view'], path: template.path.map((part) => part.kind === 'thread' ? { ...part, id, name: `Named ${id}` } : part) };
}
export function nativeEnvironment(id: string): Environment {
  return { ...detailEnvironment, id, name: id, access: { ...detailEnvironment.access, permissions: ['environment.view', 'files.view'] } };
}
export function useNativeFeed(target: ThreadReadTarget | null, visible = true): void {
  const read = useThreadReadRef(target ?? '');
  const selection = useThreadView(read).read?.selection ?? null;
  useLayoutEffect(() => {
    if (target === null) return;
    feedCalls.push({ native: read.threadId, owner: read.key.owner, controls: read.controls === true, visible });
    const row = nativeRows.get(read.threadId), current = threadViewOf(read);
    if (row === undefined || current.thread === row && current.read?.ready === true) return;
    const captured = threadWriter.capture(read, client.session.getState().readerVersion);
    const deliver = () => threadWriter.reset(read, { thread: row, items: [], turns: [], segments: [], projection: {
      viewed_continuation_id: captured.scope.selection ?? 'active-history', active_continuation_id: 'active-history',
      follows_active: captured.scope.selection === null, projection_revision: 1, execution_revision: 1,
    } }, false, captured);
    if (heldFeeds.has(read.threadId)) delayedReads.set(read.key.owner, deliver);
    else deliver();
  }, [target, read, selection, visible]);
}

export function FullThread({ threadId, continuation }: ThreadProps) {
  const page = usePageContext(), read = useThreadReadRef(threadId), view = useThreadView(read);
  const [draft, setDraft] = useState('');
  return <section data-testid={`full-thread-${page?.id}`} data-native={threadId} data-owner={read.key.owner} data-selection={continuation ?? 'active'}>
    <input aria-label={`Shared-style composer ${threadId}`} value={draft} onChange={(event) => setDraft(event.currentTarget.value)} />
    <output>{view.thread?.name}</output>
  </section>;
}
export function FullDocument({ documentId, block, version }: DocumentPageProps) {
  const page = usePageContext();
  return <article data-testid={`full-document-${page?.id}`} data-document={documentId} data-block={block} data-version={version}><input aria-label="Full document editing" /></article>;
}
export function FullChat({ chatId, message }: { chatId: string; message?: string | null }) {
  const page = usePageContext();
  return <section data-testid={`full-chat-${page?.id}`} data-chat={chatId} data-message={message}><input aria-label="Full chat composer" /></section>;
}
export function FileWitness({ tab }: { tab: { id: string; of?: Record<string, string> } }) {
  return <article data-testid={`file-${tab.id}`} data-path={tab.of?.path} data-environment={tab.of?.env} data-line={tab.of?.line}>Full file</article>;
}
export function InspectorWitness() {
  const instance = useViewInstance(), id = instance?.target?.kind === 'thread' ? instance.target.id : '';
  const read = useThreadReadRef(id), view = useThreadView(read);
  useNativeFeed(id === '' ? null : read, false);
  return <output data-testid={`inspector-${instance?.id}`} data-native={id} data-owner={read.key.owner} data-controls={String(read.controls === true)}>{view.read?.selection ?? 'active'}</output>;
}
