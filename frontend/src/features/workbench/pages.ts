/** The existing full route bodies are the workbench page adapters; auth/aliases stay at the outer router. */
import { prepareModuleWords } from '@/i18n/wordRuntime';
import { Route as ChatChatIdPage } from '@/routes/chat/$chatId/index';
import { Route as PlaceChatNodeIdPage } from '@/routes/place-chat/$nodeId/index';
import { Route as HomePage } from '@/routes/index';
import { Route as AdminPage } from '@/routes/admin/index';
import { Route as AdminSectionPage } from '@/routes/admin/$section/index';
import { Route as ApprovalsPage } from '@/routes/approvals/index';
import { Route as DocDocumentIdPage } from '@/routes/doc/$documentId/index';
import { Route as DocumentFolderFolderIdPage } from '@/routes/document-folder/$folderId/index';
import { Route as DocumentsNodeIdPage } from '@/routes/documents/$nodeId/index';
import { Route as EverythingPage } from '@/routes/everything/index';
import { Route as HelpArticlePage } from '@/routes/help/$article/index';
import { Route as HelpPage } from '@/routes/help/index';
import { Route as InboxPage } from '@/routes/inbox/index';
import { Route as MessagesIdPage } from '@/routes/messages/$id/index';
import { Route as InspectKindIdPage } from '@/routes/inspect/$kind/$id/index';
import { Route as OrgSlugPage } from '@/routes/org/$slug/index';
import { Route as OrgSlugMessagesPage } from '@/routes/org/$slug/messages/index';
import { Route as OrgSlugSettingsSectionPage } from '@/routes/org/$slug/settings/$section/index';
import { Route as ProjectProjectIdPage } from '@/routes/project/$projectId/index';
import { Route as ProjectProjectIdSettingsPage } from '@/routes/project/$projectId/settings/index';
import { Route as ResourcesAgentsIdPage } from '@/routes/resources/agents/$id/index';
import { Route as ResourcesAgentsPage } from '@/routes/resources/agents/index';
import { Route as ResourcesEnvironmentsIdPage } from '@/routes/resources/environments/$id/index';
import { Route as ResourcesEnvironmentsPage } from '@/routes/resources/environments/index';
import { Route as ResourcesHostsIdPage } from '@/routes/resources/hosts/$id/index';
import { Route as ResourcesHostsPage } from '@/routes/resources/hosts/index';
import { Route as ResourcesPage } from '@/routes/resources/index';
import { Route as ResourcesMachinesIdPage } from '@/routes/resources/machines/$id/index';
import { Route as ResourcesMachinesPage } from '@/routes/resources/machines/index';
import { Route as ResourcesMcpIdPage } from '@/routes/resources/mcp/$id/index';
import { Route as ResourcesMcpPage } from '@/routes/resources/mcp/index';
import { Route as ResourcesMountsIdPage } from '@/routes/resources/mounts/$id/index';
import { Route as ResourcesMountsPage } from '@/routes/resources/mounts/index';
import { Route as ResourcesPoolsIdPage } from '@/routes/resources/pools/$id/index';
import { Route as ResourcesPoolsPage } from '@/routes/resources/pools/index';
import { Route as ResourcesProvidersIdPage } from '@/routes/resources/providers/$id/index';
import { Route as ResourcesSkillsIdPage } from '@/routes/resources/skills/$id/index';
import { Route as ResourcesSkillsPage } from '@/routes/resources/skills/index';
import { Route as ResourcesProvidersPage } from '@/routes/resources/providers/index';
import { Route as ResourcesTemplatesIdPage } from '@/routes/resources/templates/$id/index';
import { Route as ResourcesTemplatesPage } from '@/routes/resources/templates/index';
import { Route as SearchPage } from '@/routes/search/index';
import { Route as SettingsExportsPage } from '@/routes/settings/exports/index';
import { Route as SettingsImportsPage } from '@/routes/settings/imports/index';
import { Route as SettingsInterfacePage } from '@/routes/settings/interface/index';
import { Route as SettingsNotificationsPage } from '@/routes/settings/notifications/index';
import { Route as SettingsProfilePage } from '@/routes/settings/profile/index';
import { Route as SettingsSecurityPage } from '@/routes/settings/security/index';
import { Route as SettingsTokensPage } from '@/routes/settings/tokens/index';
import { Route as SettingsUsagePage } from '@/routes/settings/usage/index';
import { Route as SettingsYourPagePage } from '@/routes/settings/your-page/index';
import { Route as SharedPage } from '@/routes/shared/index';
import { Route as StoragePage } from '@/routes/storage/index';
import { Route as ThreadThreadIdPage } from '@/routes/thread/$threadId/index';
import { Route as TopicTopicIdPage } from '@/routes/topic/$topicId/index';
import { Route as TraceNodeIdPage } from '@/routes/trace/$nodeId/index';
import { Route as UUsernamePage } from '@/routes/u/$username';
import { Route as WorkbenchPage } from '@/routes/workbench/index';
import { Route as UsagePage } from '@/routes/usage/index';
import type { AnyRoute } from '@tanstack/react-router';
import type { ComponentType } from 'react';
import type { ScreenKind } from '@/routes/-access';
import type { PageRoute } from './pageTargets';
export { locatePage, pageParam, type PageRoute, type PageTarget } from './pageTargets';

const pages = {
  '/chat/$chatId/': [ChatChatIdPage, 'routes/chat/$chatId/index.tsx'],
  '/place-chat/$nodeId/': [PlaceChatNodeIdPage, 'routes/place-chat/$nodeId/index.tsx'],
  '/': [HomePage, 'routes/index.tsx'],
  '/admin/': [AdminPage, 'routes/admin/index.tsx'],
  '/admin/$section/': [AdminSectionPage, 'routes/admin/$section/index.tsx'],
  '/approvals/': [ApprovalsPage, 'routes/approvals/index.tsx'],
  '/doc/$documentId/': [DocDocumentIdPage, 'routes/doc/$documentId/index.tsx'],
  '/document-folder/$folderId/': [DocumentFolderFolderIdPage, 'routes/document-folder/$folderId/index.tsx'],
  '/documents/$nodeId/': [DocumentsNodeIdPage, 'routes/documents/$nodeId/index.tsx'],
  '/everything/': [EverythingPage, 'routes/everything/index.tsx'],
  '/help/$article/': [HelpArticlePage, 'routes/help/$article/index.tsx'],
  '/help/': [HelpPage, 'routes/help/index.tsx'],
  '/inbox/': [InboxPage, 'routes/inbox/index.tsx'],
  '/messages/$id/': [MessagesIdPage, 'routes/messages/$id/index.tsx'],
  '/inspect/$kind/$id/': [InspectKindIdPage, 'routes/inspect/$kind/$id/index.tsx'],
  '/org/$slug/': [OrgSlugPage, 'routes/org/$slug/index.tsx'],
  '/org/$slug/messages/': [OrgSlugMessagesPage, 'routes/org/$slug/messages/index.tsx'],
  '/org/$slug/settings/$section/': [OrgSlugSettingsSectionPage, 'routes/org/$slug/settings/$section/index.tsx'],
  '/project/$projectId/': [ProjectProjectIdPage, 'routes/project/$projectId/index.tsx'],
  '/project/$projectId/settings/': [ProjectProjectIdSettingsPage, 'routes/project/$projectId/settings/index.tsx'],
  '/resources/agents/$id/': [ResourcesAgentsIdPage, 'routes/resources/agents/$id/index.tsx'],
  '/resources/agents/': [ResourcesAgentsPage, 'routes/resources/agents/index.tsx'],
  '/resources/environments/$id/': [ResourcesEnvironmentsIdPage, 'routes/resources/environments/$id/index.tsx'],
  '/resources/environments/': [ResourcesEnvironmentsPage, 'routes/resources/environments/index.tsx'],
  '/resources/hosts/$id/': [ResourcesHostsIdPage, 'routes/resources/hosts/$id/index.tsx'],
  '/resources/hosts/': [ResourcesHostsPage, 'routes/resources/hosts/index.tsx'],
  '/resources/': [ResourcesPage, 'routes/resources/index.tsx'],
  '/resources/machines/$id/': [ResourcesMachinesIdPage, 'routes/resources/machines/$id/index.tsx'],
  '/resources/machines/': [ResourcesMachinesPage, 'routes/resources/machines/index.tsx'],
  '/resources/mcp/$id/': [ResourcesMcpIdPage, 'routes/resources/mcp/$id/index.tsx'],
  '/resources/mcp/': [ResourcesMcpPage, 'routes/resources/mcp/index.tsx'],
  '/resources/mounts/$id/': [ResourcesMountsIdPage, 'routes/resources/mounts/$id/index.tsx'],
  '/resources/mounts/': [ResourcesMountsPage, 'routes/resources/mounts/index.tsx'],
  '/resources/pools/$id/': [ResourcesPoolsIdPage, 'routes/resources/pools/$id/index.tsx'],
  '/resources/pools/': [ResourcesPoolsPage, 'routes/resources/pools/index.tsx'],
  '/resources/providers/$id/': [ResourcesProvidersIdPage, 'routes/resources/providers/$id/index.tsx'],
  '/resources/skills/$id/': [ResourcesSkillsIdPage, 'routes/resources/skills/$id/index.tsx'],
  '/resources/skills/': [ResourcesSkillsPage, 'routes/resources/skills/index.tsx'],
  '/resources/providers/': [ResourcesProvidersPage, 'routes/resources/providers/index.tsx'],
  '/resources/templates/$id/': [ResourcesTemplatesIdPage, 'routes/resources/templates/$id/index.tsx'],
  '/resources/templates/': [ResourcesTemplatesPage, 'routes/resources/templates/index.tsx'],
  '/search/': [SearchPage, 'routes/search/index.tsx'],
  '/settings/exports/': [SettingsExportsPage, 'routes/settings/exports/index.tsx'],
  '/settings/imports/': [SettingsImportsPage, 'routes/settings/imports/index.tsx'],
  '/settings/interface/': [SettingsInterfacePage, 'routes/settings/interface/index.tsx'],
  '/settings/notifications/': [SettingsNotificationsPage, 'routes/settings/notifications/index.tsx'],
  '/settings/profile/': [SettingsProfilePage, 'routes/settings/profile/index.tsx'],
  '/settings/security/': [SettingsSecurityPage, 'routes/settings/security/index.tsx'],
  '/settings/tokens/': [SettingsTokensPage, 'routes/settings/tokens/index.tsx'],
  '/settings/usage/': [SettingsUsagePage, 'routes/settings/usage/index.tsx'],
  '/settings/your-page/': [SettingsYourPagePage, 'routes/settings/your-page/index.tsx'],
  '/shared/': [SharedPage, 'routes/shared/index.tsx'],
  '/storage/': [StoragePage, 'routes/storage/index.tsx'],
  '/thread/$threadId/': [ThreadThreadIdPage, 'routes/thread/$threadId/index.tsx'],
  '/topic/$topicId/': [TopicTopicIdPage, 'routes/topic/$topicId/index.tsx'],
  '/trace/$nodeId/': [TraceNodeIdPage, 'routes/trace/$nodeId/index.tsx'],
  '/u/$username': [UUsernamePage, 'routes/u/$username.tsx'],
  '/workbench/': [WorkbenchPage, 'routes/workbench/index.tsx'],
  '/usage/': [UsagePage, 'routes/usage/index.tsx'],
} as const satisfies Record<PageRoute, readonly [unknown, string]>;

export interface PageDefinition {
  route: AnyRoute;
  Body: ComponentType;
  screen: ScreenKind;
  validate: (search: Record<string, unknown>) => Record<string, unknown>;
}
const loaded = new Map<PageRoute, Promise<PageDefinition>>();

export function pageDefinition(id: PageRoute): Promise<PageDefinition> {
  let pending = loaded.get(id);
  if (pending === undefined) {
    // These are the same route objects the outer router already loaded; only their bodies are lazy.
    const [registered, words] = pages[id];
    pending = prepareModuleWords(words).then(() => {
      const route = registered as AnyRoute;
      const Body = route.options.component;
      if (Body === undefined) throw new Error('This address has no content view.');
      const validate: unknown = route.options.validateSearch;
      return { route, Body, screen: route.options.staticData?.screen ?? 'page',
        validate: (search: Record<string, unknown>) => {
          const result: unknown = typeof validate === 'function' ? (validate as (value: Record<string, unknown>) => unknown)(search) : search;
          if (typeof result !== 'object' || result === null || Array.isArray(result)) throw new Error('Invalid page search.');
          return result as Record<string, unknown>;
        } };
    });
    loaded.set(id, pending);
    void pending.catch(() => { if (loaded.get(id) === pending) loaded.delete(id); });
  }
  return pending;
}
