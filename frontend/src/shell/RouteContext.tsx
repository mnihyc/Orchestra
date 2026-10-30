import { threadIconFacts } from '@/ui/ThreadState';
/** The addressed resource supplies frame context without mounting a hidden Screen body. */
import type { InspectorTarget } from '@/features/workbench/layout';
import { environmentQuery } from '@/api/workspace';
import { pageParam, type PageTarget } from '@/features/workbench/pages';
import { useContentFocused } from '@/features/workbench/instance';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { useQuery } from '@tanstack/react-query';
import { useParams, useRouterState } from '@tanstack/react-router';
import { Box, Briefcase, Diamond, FileText, Folder, LayoutGrid, MessageSquareText, Settings } from 'lucide-react';
import { useEffect, useLayoutEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { selectThreadContinuation } from '@/api/threadRead';
import { useTreeEntry } from '@/features/documents/useTreeEntry';
import { documentMetadataRefused, documentWithPlacement } from '@/features/documents/placementView';
import { DocumentsContext, FolderContext } from '@/features/documents/FolderContext';
import { nodeQuery, orgBySlugQuery } from '@/api/nodes';
import { isFolderPath } from '@/features/navigation/places';
import { isThreadForScreen } from '@/features/thread/cached';
import { allTabs } from '@/features/workbench/layout';
import { useTabName, useWorkbench } from '@/features/workbench/store';
import { holds } from '@/lib/permissions';
import { useDocumentFeed, useThreadFeed } from '@/realtime/live';
import { useDocumentState } from '@/realtime/stores/document';
import { useThreadView } from '@/realtime/stores/thread';
import { selectOrg } from '@/shell/org';
import { useCurrentHeading, useRouteHeading } from '@/shell/screen';
import { useSettingsTabTitle } from '@/features/workbench/settingsTabTitle';
import { sectionLabel } from '@/features/settings/sectionLabels';
import { settingsNavigation } from '@/features/settings/navigation';
import { orgPageTitle } from '@/features/org/title';

function ThreadContext({ id, continuation }: { id: string; continuation: string | null }) {
  const { t } = useTranslation();
  const user = useStore(client.session, (state) => state.userId);
  const focusedContent = useContentFocused();
  const read = useThreadReadRef(id);
  useLayoutEffect(() => selectThreadContinuation(read, continuation), [read, continuation]);
  // The frame needs live identity and permissions, but does not claim its conversation was read.
  useThreadFeed(id, false);
  const view = useThreadView(id);
  const thread = view.closed === null && isThreadForScreen(view.thread, id) ? view.thread : null;
  useRouteHeading({ title: view.closed === null ? thread?.name ?? t('thread.screen.title') : t('craft.threadUnavailable'),
    icon: MessageSquareText, thread: threadIconFacts(thread), nodeId: thread?.id, path: thread?.path,
    target: thread === null ? undefined : { kind: 'thread', id },
    views: thread === null && view.error === null && view.closed === null ? undefined : {
      ready: thread !== null, workspace: thread?.environment !== null && thread?.environment_permissions.includes('files.view') === true,
      terminal: thread?.environment_permissions.includes('terminal.manage') === true,
      processes: thread?.environment_permissions.includes('terminal.view') === true,
    } });
  const orgId = thread?.path[0]?.redacted === false ? thread.path[0].id : undefined;
  useEffect(() => { if (focusedContent && orgId !== undefined) selectOrg(user, orgId); }, [user, orgId, focusedContent]);
  return null;
}

function NodeContext({ id, kind, settings, settingsSection = 'general' }: { id: string; kind: 'org' | 'project' | 'topic'; settings: boolean; settingsSection?: string | undefined }) {
  const { t } = useTranslation();
  const user = useStore(client.session, (state) => state.userId);
  const focusedContent = useContentFocused();
  const query = useQuery(nodeQuery(id));
  const node = !query.isError && query.data?.kind === kind ? query.data : undefined;
  const tabTitle = useSettingsTabTitle(sectionLabel(settingsSection, t), settings ? node : undefined);
  useRouteHeading({ title: settings ? t('screens.sections.settings') : node?.kind === 'org' ? orgPageTitle(node, t) : node?.name ?? t(`screens.${kind}.title`),
    tabTitle: settings ? tabTitle : undefined, tabPending: query.isPending,
    icon: settings ? Settings : kind === 'org' ? LayoutGrid : kind === 'topic' ? Diamond : node !== undefined && isFolderPath(node.path) ? Folder : Briefcase,
    nodeId: node?.id, path: node?.path, leaf: settings,
    target: node === undefined ? undefined : { kind, id },
    views: query.isPending ? undefined : { ready: node !== undefined, chat: node !== undefined && holds(node.access, 'chat.view') } });
  const orgId = node?.path[0]?.redacted === false ? node.path[0].id : undefined;
  useEffect(() => { if (focusedContent && orgId !== undefined) selectOrg(user, orgId); }, [user, orgId, focusedContent]);
  return null;
}

function OrgContext({ slug }: { slug: string }) {
  const { t } = useTranslation();
  const user = useStore(client.session, (state) => state.userId);
  const focusedContent = useContentFocused();
  const query = useQuery(orgBySlugQuery(slug));
  const org = query.isError ? undefined : query.data;
  useRouteHeading({ title: org === undefined ? t('screens.org.title') : orgPageTitle(org, t), tabPending: query.isPending, icon: LayoutGrid, nodeId: org?.id, path: org?.path,
    target: org === undefined ? undefined : { kind: 'org', id: org.id },
    views: query.isPending ? undefined : { ready: org !== undefined, chat: org?.org_kind !== 'personal' && org !== undefined && holds(org.access, 'chat.view') } });
  const orgId = org?.path[0]?.redacted === false ? org.path[0].id : undefined;
  useEffect(() => { if (focusedContent && orgId !== undefined) selectOrg(user, orgId); }, [user, orgId, focusedContent]);
  return null;
}

function DocumentContext({ id }: { id: string }) {
  const { t } = useTranslation();
  useDocumentFeed(id);
  const state = useDocumentState(id);
  const metadata = useTreeEntry({ kind: 'document', id }, false, state.document !== null && state.ended === null && !state.document.deleted);
  const doc = state.ended === null && !documentMetadataRefused(state.error) && metadata.isSuccess ? documentWithPlacement(state.document, metadata.data) : null;
  const place = useQuery({ ...nodeQuery(doc?.node.id ?? ''), enabled: doc !== null && !doc.node.redacted }).data;
  useRouteHeading({ title: doc?.title ?? (state.ended !== null || state.error !== null || metadata.isError ? t('craft.documentUnavailable') : t('craftLoading.document')),
    icon: FileText, nodeId: doc?.node.id, path: doc?.path, leaf: true,
    target: doc === null || doc.deleted ? undefined : { kind: 'document', id },
    views: doc === null && state.ended === null && state.error === null && !metadata.isError ? undefined : { ready: doc !== null, chat: place !== undefined && holds(place.access, 'chat.view') } });
  return null;
}

function EnvironmentContext({ id }: { id: string }) {
  const { t } = useTranslation();
  const query = useQuery(environmentQuery(id));
  const environment = query.isError ? undefined : query.data;
  useRouteHeading({ title: environment?.name ?? t('views.environment.title'), icon: Box,
    target: environment === undefined ? undefined : { kind: 'environment', id },
    views: { ready: environment !== undefined, workspace: environment?.access.permissions.includes('files.view') === true,
      terminal: environment?.access.permissions.includes('terminal.manage') === true, processes: environment?.access.permissions.includes('terminal.view') === true } });
  return null;
}

/** Only root resource pages own this context; Settings and other subpages retain their own heading. */
export function RouteContext() {
  const params = useParams({ strict: false });
  const location = useRouterState({ select: (state) => state.location });
  const path = location.pathname.replace(/\/$/, '');
  const navigation = settingsNavigation(path, location.search);
  const continuation = (location.search as Record<string, unknown>).continuation;
  if (/^\/thread\/[^/]+$/.test(path) && typeof params.threadId === 'string') return <ThreadContext key={path} id={params.threadId}
    continuation={typeof continuation === 'string' && continuation !== '' ? continuation : null} />;
  if (/^\/project\/[^/]+$/.test(path) && typeof params.projectId === 'string') return <NodeContext key={path} id={params.projectId} kind="project" settings={false} />;
  if (/^\/topic\/[^/]+$/.test(path) && typeof params.topicId === 'string') return <NodeContext key={path} id={params.topicId} kind="topic" settings={(location.search as Record<string, unknown>).tab === 'settings'} settingsSection={navigation?.kind === 'topic' ? navigation.current : undefined} />;
  if (/^\/org\/[^/]+$/.test(path) && typeof params.slug === 'string') return <OrgContext key={path} slug={params.slug} />;
  if (/^\/doc\/[^/]+$/.test(path) && typeof params.documentId === 'string') return <DocumentContext key={path} id={params.documentId} />;
  if (/^\/document-folder\/[^/]+$/.test(path) && typeof params.folderId === 'string') return <FolderContext key={path} id={params.folderId} />;
  if (/^\/inspect\/document_container\/[^/]+$/.test(path) && typeof params.id === 'string') return <FolderContext key={path} id={params.id} retained />;
  if (/^\/documents\/[^/]+$/.test(path) && typeof params.nodeId === 'string') return <DocumentsContext key={path} id={params.nodeId} />;
  return null;
}

/** Fixed panels use the resource's actual ID, including orgs whose URL uses a different address. */
export function InspectorContext({ target, continuation = null }: { target: InspectorTarget; continuation?: string | null }) {
  if (target.kind === 'thread') return <ThreadContext id={target.id} continuation={continuation} />;
  if (target.kind === 'document') return <DocumentContext id={target.id} />;
  if (target.kind === 'environment') return <EnvironmentContext id={target.id} />;
  return <NodeContext id={target.id} kind={target.kind} settings={false} />;
}

/** The same resource metadata supplier, addressed by the owning content or fixed panel. */
export function TargetContext({ target }: { target: PageTarget }) {
  const { route, params, search } = target;
  const navigation = settingsNavigation(target.pathname, search);
  if (route === '/resources/environments/$id/') return <EnvironmentContext id={pageParam(target, 'id')} />;
  if (route === '/thread/$threadId/') return <ThreadContext id={pageParam(target, 'threadId')} continuation={typeof search.continuation === 'string' ? search.continuation : null} />;
  if (route === '/doc/$documentId/') return <DocumentContext id={pageParam(target, 'documentId')} />;
  if (route === '/project/$projectId/') return <NodeContext id={pageParam(target, 'projectId')} kind="project" settings={false} />;
  if (route === '/topic/$topicId/') return <NodeContext id={pageParam(target, 'topicId')} kind="topic" settings={search.tab === 'settings'} settingsSection={navigation?.kind === 'topic' ? navigation.current : undefined} />;
  if (route === '/org/$slug/') return <OrgContext slug={pageParam(target, 'slug')} />;
  if (route === '/document-folder/$folderId/') return <FolderContext id={pageParam(target, 'folderId')} />;
  if (route === '/documents/$nodeId/') return <DocumentsContext id={pageParam(target, 'nodeId')} />;
  if (route === '/inspect/$kind/$id/' && params.kind === 'document_container') return <FolderContext id={pageParam(target, 'id')} retained />;
  return null;
}

function NamedScreen({ id }: { id: string }) {
  const heading = useCurrentHeading();
  useTabName(id, heading?.tabTitle ?? heading?.title, heading?.icon, undefined, heading === null || heading.tabPending === true, heading?.thread);
  return null;
}

/** Tab identity is frame metadata, so the initially inactive Screen also gets its real name. */
export function RouteTabNames() {
  const layout = useWorkbench((state) => state.layout);
  return allTabs(layout).filter((tab) => tab.view === 'screen').map((tab) => <NamedScreen key={tab.id} id={tab.id} />);
}
