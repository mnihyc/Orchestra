/** Old file links are resolved once by their addressed content owner, never by a contextual panel. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef } from 'react';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { environmentQuery } from '@/api/workspace';
import { openFile } from '@/features/files/FileTabs';
import { useContentFocused } from './instance';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { threadViewOf, useThreadView } from '@/realtime/stores/thread';
import { isCurrentThreadRead } from '@/api/threadRead';
import { fileLine, relativeFilePath } from './contentAddress';
import { findTab, type Tab } from './layout';
import { pageParam, type PageTarget } from './pages';
import { useWorkbench } from './store';

interface Intent { owner: string; href: string; path: string; line: number | undefined }
function stillOwned(intent: Intent, reader: number): boolean {
  const state = useWorkbench.getState(), tab = findTab(state.layout, intent.owner)?.tab;
  return (client.session.getState().readerVersion ?? 0) === reader && state.activeContent === intent.owner
    && tab?.view === 'page' && tab.of?.href === intent.href;
}
/** Only the raw address belonging to this parsed page may carry the legacy intent. */
export function PageFileIntent({ tab, target }: { tab: Tab; target: PageTarget }) {
  if (tab.of?.href === undefined) return null;
  const parameters = new URL(tab.of.href, 'https://orch.invalid').searchParams;
  const ownFile = target.search.file;
  const file = typeof ownFile === 'string' ? ownFile : parameters.get('file');
  if (file === null || !relativeFilePath(file) || parameters.getAll('file').length !== 1 || parameters.getAll('line').length > 1) return null;
  const rawLine = parameters.get('line'), line = fileLine(target.search.line ?? rawLine);
  if (rawLine !== null && line === undefined) return null;
  const intent = { owner: tab.id, href: tab.of.href, path: file, line };
  if (target.route === '/thread/$threadId/') return <ThreadFileIntent intent={intent} threadId={pageParam(target, 'threadId')} />;
  if (target.route === '/resources/environments/$id/') return <EnvironmentFileIntent intent={intent} environmentId={pageParam(target, 'id')} />;
  return null;
}

function ThreadFileIntent({ intent, threadId }: { intent: Intent; threadId: string }) {
  const focused = useContentFocused();
  const reader = useStore(client.session, (state) => state.readerVersion ?? 0);
  const read = useThreadReadRef(threadId), view = useThreadView(read);
  const acknowledged = useRef<{ href: string; reader: number; lifetime: number; environmentId: string } | null>(null);
  useEffect(() => {
    const scope = view.read, environmentId = view.thread?.environment?.id;
    if (!focused || !stillOwned(intent, reader) || scope === null || !scope.ready || scope.reader !== reader
      || view.closed !== null || view.error !== null || environmentId === undefined
      || acknowledged.current?.href === intent.href && acknowledged.current.reader === reader) return;
    const current = threadViewOf(read);
    if (!isCurrentThreadRead({ ref: read, threadId, scope })
      || current.closed !== null || current.thread?.id !== threadId || current.thread.environment?.id !== environmentId) return;
    acknowledged.current = { href: intent.href, reader, lifetime: scope.lifetime, environmentId };
    openFile({ environmentId, threadId, path: intent.path, line: intent.line, history: 'replace' });
  }, [focused, intent, reader, read, threadId, view.read, view.thread, view.closed, view.error]);
  return null;
}

function EnvironmentFileIntent({ intent, environmentId }: { intent: Intent; environmentId: string }) {
  const focused = useContentFocused();
  const reader = useStore(client.session, (state) => state.readerVersion ?? 0);
  const cache = useQueryClient();
  const query = useQuery({ ...environmentQuery(environmentId), enabled: focused });
  const acknowledged = useRef<{ href: string; reader: number; environmentId: string } | null>(null);
  useEffect(() => {
    const environment = query.data;
    if (!focused || !stillOwned(intent, reader) || environment?.id !== environmentId || query.error !== null
      || acknowledged.current?.href === intent.href && acknowledged.current.reader === reader
      || cache.getQueryData(environmentQuery(environmentId).queryKey) !== environment) return;
    acknowledged.current = { href: intent.href, reader, environmentId };
    openFile({ environmentId, threadId: '', path: intent.path, line: intent.line, history: 'replace' });
  }, [focused, intent, reader, environmentId, query.data, query.error, cache]);
  return null;
}
