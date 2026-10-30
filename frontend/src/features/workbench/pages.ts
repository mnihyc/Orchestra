/** The existing full route bodies are the workbench page adapters; auth/aliases stay at the outer router. */
import type { AnyRoute } from '@tanstack/react-router';
import type { ComponentType } from 'react';
import type { ScreenKind } from '@/routes/-access';
import type { PageRoute } from './pageTargets';
export { locatePage, pageParam, type PageRoute, type PageTarget } from './pageTargets';

const pages = {
  "/chat/$chatId/": () => import("@/routes/chat/$chatId/index"),
  "/place-chat/$nodeId/": () => import("@/routes/place-chat/$nodeId/index"),
  "/": () => import("@/routes/index"),
  "/admin/": () => import("@/routes/admin/index"),
  "/admin/$section/": () => import("@/routes/admin/$section/index"),
  "/approvals/": () => import("@/routes/approvals/index"),
  "/doc/$documentId/": () => import("@/routes/doc/$documentId/index"),
  "/document-folder/$folderId/": () => import("@/routes/document-folder/$folderId/index"),
  "/documents/$nodeId/": () => import("@/routes/documents/$nodeId/index"),
  "/everything/": () => import("@/routes/everything/index"),
  "/help/$article/": () => import("@/routes/help/$article/index"),
  "/help/": () => import("@/routes/help/index"),
  "/inbox/": () => import("@/routes/inbox/index"),
  "/inspect/$kind/$id/": () => import("@/routes/inspect/$kind/$id/index"),
  "/org/$slug/": () => import("@/routes/org/$slug/index"),
  "/org/$slug/messages/": () => import("@/routes/org/$slug/messages/index"),
  "/org/$slug/settings/$section/": () => import("@/routes/org/$slug/settings/$section/index"),
  "/project/$projectId/": () => import("@/routes/project/$projectId/index"),
  "/project/$projectId/settings/": () => import("@/routes/project/$projectId/settings/index"),
  "/resources/agents/$id/": () => import("@/routes/resources/agents/$id/index"),
  "/resources/agents/": () => import("@/routes/resources/agents/index"),
  "/resources/environments/$id/": () => import("@/routes/resources/environments/$id/index"),
  "/resources/environments/": () => import("@/routes/resources/environments/index"),
  "/resources/hosts/$id/": () => import("@/routes/resources/hosts/$id/index"),
  "/resources/hosts/": () => import("@/routes/resources/hosts/index"),
  "/resources/": () => import("@/routes/resources/index"),
  "/resources/machines/$id/": () => import("@/routes/resources/machines/$id/index"),
  "/resources/machines/": () => import("@/routes/resources/machines/index"),
  "/resources/mcp/$id/": () => import("@/routes/resources/mcp/$id/index"),
  "/resources/mcp/": () => import("@/routes/resources/mcp/index"),
  "/resources/mounts/$id/": () => import("@/routes/resources/mounts/$id/index"),
  "/resources/mounts/": () => import("@/routes/resources/mounts/index"),
  "/resources/providers/$id/": () => import("@/routes/resources/providers/$id/index"),
  "/resources/skills/$id/": () => import("@/routes/resources/skills/$id/index"),
  "/resources/skills/": () => import("@/routes/resources/skills/index"),
  "/resources/providers/": () => import("@/routes/resources/providers/index"),
  "/resources/templates/$id/": () => import("@/routes/resources/templates/$id/index"),
  "/resources/templates/": () => import("@/routes/resources/templates/index"),
  "/search/": () => import("@/routes/search/index"),
  "/settings/exports/": () => import("@/routes/settings/exports/index"),
  "/settings/imports/": () => import("@/routes/settings/imports/index"),
  "/settings/interface/": () => import("@/routes/settings/interface/index"),
  "/settings/notifications/": () => import("@/routes/settings/notifications/index"),
  "/settings/profile/": () => import("@/routes/settings/profile/index"),
  "/settings/security/": () => import("@/routes/settings/security/index"),
  "/settings/tokens/": () => import("@/routes/settings/tokens/index"),
  "/settings/usage/": () => import("@/routes/settings/usage/index"),
  "/settings/your-page/": () => import("@/routes/settings/your-page/index"),
  "/shared/": () => import("@/routes/shared/index"),
  "/storage/": () => import("@/routes/storage/index"),
  "/thread/$threadId/": () => import("@/routes/thread/$threadId/index"),
  "/topic/$topicId/": () => import("@/routes/topic/$topicId/index"),
  "/trace/$nodeId/": () => import("@/routes/trace/$nodeId/index"),
  "/u/$username": () => import("@/routes/u/$username"),
  "/workbench/": () => import("@/routes/workbench/index"),
  "/usage/": () => import("@/routes/usage/index"),
} as const satisfies Record<PageRoute, () => Promise<unknown>>;

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
    pending = pages[id]().then(({ Route }) => {
      const route = Route as AnyRoute;
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
