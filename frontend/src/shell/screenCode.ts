/** Intent fetches only route code, without invoking a loader, admission side effect or data read. */
import { useRouter, type AnyRoute, type AnyRouter } from '@tanstack/react-router';
import { useEffect } from 'react';

type CodeRouter = Pick<AnyRouter, 'matchRoutes' | 'routesById' | 'loadRouteChunk'>;

export function prefetchScreenCode(router: CodeRouter, href: string): void {
  let url: URL;
  try { url = new URL(href, window.location.href); } catch { return; }
  if (url.origin !== window.location.origin || !['http:', 'https:'].includes(url.protocol)) return;
  const search: Record<string, string> = {};
  url.searchParams.forEach((value, key) => { search[key] = value; });
  // AnyRouter erases the tree's mapped route values. Its native route table still holds routes.
  const routes = router.routesById as Readonly<Record<string, AnyRoute | undefined>>;
  for (const match of router.matchRoutes(url.pathname, search)) {
    if (typeof match.routeId !== 'string') continue;
    const route = routes[match.routeId];
    if (route !== undefined) void router.loadRouteChunk(route)?.catch(() => undefined);
  }
}

export function useScreenCodePrefetch(): void {
  const router = useRouter();
  useEffect(() => {
    const prepare = (event: Event) => {
      const link = event.target instanceof Element ? event.target.closest<HTMLAnchorElement>('a[href]') : null;
      if (link !== null && !link.hasAttribute('download')) prefetchScreenCode(router, link.href);
    };
    document.addEventListener('pointerover', prepare);
    document.addEventListener('focusin', prepare);
    return () => { document.removeEventListener('pointerover', prepare); document.removeEventListener('focusin', prepare); };
  }, [router]);
}
