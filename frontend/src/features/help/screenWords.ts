/** Screen help uses the most specific registered route, including its detail pages. */
import type { TranslationKey } from '@/i18n';

const SCREEN_ABOUT: Readonly<Partial<Record<string, TranslationKey>>> = {
  '/': 'screens.home.about',
  '/workbench': 'screens.home.about',
  '/chat/$chatId': 'screens.messages.about',
  '/place-chat/$nodeId': 'screens.messages.about',
  '/inbox': 'screens.inbox.about',
  '/approvals': 'screens.approvals.about',
  '/shared': 'screens.shared.about',
  '/everything': 'screens.everything.about',
  '/search': 'screens.search.about',
  '/help': 'screens.help.about',
  '/help/$article': 'screens.helpArticle.about',
  '/usage': 'screens.usage.about',
  '/storage': 'screens.storage.about',
  '/org/$slug': 'screens.org.about',
  '/org/$slug/messages': 'screens.messages.about',
  '/org/$slug/settings/$section': 'screens.orgSettings.about',
  '/project/$projectId': 'screens.project.about',
  '/project/$projectId/settings': 'screens.projectSettings.about',
  '/topic/$topicId': 'screens.topic.about',
  '/thread/$threadId': 'screens.thread.about',
  '/doc/$documentId': 'screens.document.about',
  '/document-folder/$folderId': 'documentFolders.about',
  '/documents/$nodeId': 'documentFolders.about',
  '/resources': 'screens.resources.about',
  '/resources/providers': 'screens.providers.about',
  '/resources/agents': 'screens.agents.about',
  '/resources/environments': 'screens.environments.about',
  '/resources/hosts': 'screens.hosts.about',
  '/resources/machines': 'screens.machines.about',
  '/resources/templates': 'screens.templates.about',
  '/resources/mcp': 'screens.mcp.about',
  '/resources/mounts': 'screens.mounts.about',
  '/resources/skills': 'screens.skills.about',
  '/settings/profile': 'screens.profile.about',
  '/settings/your-page': 'screens.profile.about',
  '/settings/security': 'screens.security.about',
  '/settings/notifications': 'screens.notifications.about',
  '/settings/usage': 'screens.settingsUsage.about',
  '/settings/tokens': 'screens.tokens.about',
  '/settings/exports': 'screens.exports.about',
  '/settings/imports': 'screens.imports.about',
  '/settings/interface': 'screens.interface.about',
  '/admin': 'screens.admin.about',
  '/trace/$nodeId': 'screens.trace.about',
  '/inspect/$kind/$id': 'screens.inspect.about',
  '/u/$username': 'screens.person.about',
};

export function screenAbout(route: string): TranslationKey | undefined {
  let path = route.length > 1 ? route.replace(/\/$/, '') : route;
  while (path !== '') {
    const about = SCREEN_ABOUT[path];
    if (about !== undefined) return about;
    path = path.slice(0, path.lastIndexOf('/'));
  }
  return undefined;
}
