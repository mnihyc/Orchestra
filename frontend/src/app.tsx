/** Provider, routing and application composition.
 *
 * Puts the parts together: the router with its tree of routes, the one client for what is asked of
 * the API, the shared timing of tooltips, the place where notices appear, and the size and weight
 * every icon is drawn at. It also ties the session to the rest: who is signed in is what the page
 * shows as "me", their preferences are how it looks, and when they sign out or another person signs in,
 * nothing of theirs stays in the page's query cache.
 */
import { QueryClientProvider } from '@tanstack/react-query';
import { createRouter, RouterProvider } from '@tanstack/react-router';
import { LucideProvider } from 'lucide-react';

import { meKey, metaKey } from '@/api/auth';
import { client } from '@/api/client';
import { queryClient } from '@/api/query';
import { installReaderCacheBoundary } from '@/api/readerCache';
import type { Meta } from '@/generated/api-types';
import { adoptAppearance } from '@/lib/appearance';
import { routeParams } from '@/lib/routeIdentity';
import { beginOpening } from '@/lib/opening';
import '@/realtime/live';
import { routeTree } from '@/routeTree.gen';
import { ScreenError } from '@/shell/ScreenError';
import { Toaster } from '@/ui/Toast';
import { TooltipProvider } from '@/ui/Tooltip';
import { ScreenLoading } from '@/shell/ScreenLoading';

/** Rows and buttons use 16 px icons with a 1.75 stroke; the navigation rail uses 20 px. */
const ICON_SIZE = 16;
const ICON_STROKE = 1.75;

export const router = createRouter({
  routeTree,
  // The shell prefetches route code on intent; loaders and their data never run on hover.
  defaultPreload: false,
  defaultPendingComponent: ScreenLoading,
  // A different thing at the same route gets a fresh screen, including its local forms and effects.
  defaultRemountDeps: routeParams,
  // A screen that answers quickly never flashes a skeleton first.
  defaultPendingMs: 200,
  defaultPendingMinMs: 300,
  // A screen that could not be shown says so where it would be, inside whatever stands round it.
  defaultErrorComponent: ScreenError,
  scrollRestoration: true,
});

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router;
  }
}

router.subscribe('onBeforeNavigate', () => beginOpening());

installReaderCacheBoundary(client, queryClient);
client.onSession((event) => {
  if (event.type === 'signed_out') {
    // Every route is asked again whether it is for somebody who is not signed in.
    void router.invalidate();
    return;
  }
  queryClient.setQueryData(meKey, event.result.user);
  if (event.type === 'signed_in') {
    adoptAppearance(event.result.user);
    // The shell read carries the device's zone; drawing a page does not PATCH the person.
    // The signed-in bootstrap supplies this caller's fresh permission with the shell's Meta.
    // Until it arrives, a former caller's create permission cannot remain visible.
    queryClient.setQueryData<Meta>(metaKey, (meta) => meta && { ...meta, may_create_org: false });
  }
});

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <LucideProvider size={ICON_SIZE} strokeWidth={ICON_STROKE}>
        <TooltipProvider>
          <Toaster>
            <RouterProvider router={router} />
          </Toaster>
        </TooltipProvider>
      </LucideProvider>
    </QueryClientProvider>
  );
}
