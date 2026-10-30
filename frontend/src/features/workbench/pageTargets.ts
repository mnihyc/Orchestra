/** Pure page locators for layout persistence and build tools; browser component loaders live in pages.ts. */
export const PAGE_ROUTES = [
  "/chat/$chatId/",
  "/place-chat/$nodeId/",
  "/",
  "/admin/$section/",
  "/admin/",
  "/approvals/",
  "/doc/$documentId/",
  "/document-folder/$folderId/",
  "/documents/$nodeId/",
  "/everything/",
  "/help/$article/",
  "/help/",
  "/inbox/",
  "/inspect/$kind/$id/",
  "/org/$slug/",
  "/org/$slug/messages/",
  "/org/$slug/settings/$section/",
  "/project/$projectId/",
  "/project/$projectId/settings/",
  "/resources/agents/$id/",
  "/resources/agents/",
  "/resources/environments/$id/",
  "/resources/environments/",
  "/resources/hosts/$id/",
  "/resources/hosts/",
  "/resources/",
  "/resources/machines/$id/",
  "/resources/machines/",
  "/resources/mcp/$id/",
  "/resources/mcp/",
  "/resources/mounts/$id/",
  "/resources/mounts/",
  "/resources/providers/$id/",
  "/resources/providers/",
  "/resources/skills/$id/",
  "/resources/skills/",
  "/resources/templates/$id/",
  "/resources/templates/",
  "/search/",
  "/settings/exports/",
  "/settings/imports/",
  "/settings/interface/",
  "/settings/notifications/",
  "/settings/profile/",
  "/settings/security/",
  "/settings/tokens/",
  "/settings/your-page/",
  "/settings/usage/",
  "/shared/",
  "/storage/",
  "/thread/$threadId/",
  "/topic/$topicId/",
  "/trace/$nodeId/",
  "/u/$username",
  "/usage/",
  "/workbench/",
] as const;

export type PageRoute = (typeof PAGE_ROUTES)[number];
export interface PageTarget {
  route: PageRoute;
  pathname: string;
  params: Record<string, string>;
  search: Record<string, unknown>;
  hash: string;
}
/** Match literal routes before parameterized routes; never accept external navigation or malformed segments. */
export function locatePage(pathname: string): Pick<PageTarget, 'route' | 'pathname' | 'params'> | null {
  const requested = pathname.replace(/\/$/, '') || '/';
  const aliases: Record<string, string> = { '/settings': '/settings/profile', '/settings/usage': '/usage', '/admin': '/admin/overview' };
  const path = aliases[requested] ?? requested;
  if (!path.startsWith('/') || path.startsWith('//') || path.includes('\\')) return null;
  if (path === '/') return { route: '/', pathname: '/', params: {} };
  const pieces = path.split('/');
  const patterns = [...PAGE_ROUTES].sort((a, b) => a.split('$').length - b.split('$').length);
  for (const route of patterns) {
    const pattern = route.replace(/\/$/, '').split('/');
    if (pattern.length !== pieces.length) continue;
    const params: Record<string, string> = {};
    let matches = true;
    for (let at = 0; at < pattern.length; at += 1) {
      const part = pattern[at], piece = pieces[at];
      if (part === undefined || piece === undefined) { matches = false; break; }
      if (part.startsWith('$')) {
        try { params[part.slice(1)] = decodeURIComponent(piece); } catch { matches = false; break; }
        if (params[part.slice(1)] === '') { matches = false; break; }
      } else if (part !== piece) { matches = false; break; }
    }
    if (matches) return { route, pathname: path, params };
  }
  return null;
}

/** Parameters come only from a matching route pattern, with a local failure for malformed stored targets. */
export function pageParam(target: PageTarget, name: string): string {
  const value = target.params[name];
  if (value === undefined || value === '') throw new Error('This view has an invalid resource address.');
  return value;
}
