/** Application shell and route error boundaries. Loader, permission handling and search schema are pending.
 *
 * The root of every route. Before any screen is entered it asks what the system says of itself and
 * whether this browser holds a session, and sends the visitor on where the route is not for them
 * (`-access.ts`). Then it draws what stands round the screen: the workbench for somebody signed in,
 * the plain page of the screens before sign-in otherwise. A path that no route answers, and an error
 * that no screen caught, are shown inside the same frame.
 */
import { createRootRoute, Outlet, useMatches, useRouter } from '@tanstack/react-router';
import { type ComponentProps, type ReactNode, Suspense } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { AuthFrame } from '@/features/auth/Frame';
import { ConnectionFailure } from '@/features/auth/ConnectionFailure';
import { admit, type RouteFrame } from '@/routes/-access';
import { NotFound } from '@/routes/not-found';
import { lazyPart } from '@/features/files/lazy';
import type { Shell as ShellFrame } from '@/shell/Shell';
import { ShellControls } from '@/shell/ShellControls';
import { ErrorState } from '@/ui/ErrorState';
import { ScreenLoading } from '@/shell/ScreenLoading';
import { useScreenCodePrefetch } from '@/shell/screenCode';
import { useAdminExit } from '@/shell/useAdminExit';

/** The workbench's frame, fetched when a signed-in screen is first drawn (the entry starts it with the words, see `main.tsx`). */
const Shell = lazyPart<ComponentProps<typeof ShellFrame>>(() => import('@/shell/Shell').then((module) => ({ default: module.Shell })));

export const Route = createRootRoute({
  beforeLoad: ({ location, matches }) => {
    // The deepest match is the route to be entered; the root alone means no route answers the path.
    const leaf = matches.at(-1);
    const found = leaf !== undefined && leaf.routeId !== '__root__';
    return admit({
      needs: found ? (leaf.staticData.needs ?? 'signed_in') : undefined,
      href: location.href,
      pathname: location.pathname,
      onward: (location.search as Record<string, unknown>).redirect,
    });
  },
  component: Root,
  shellComponent: Framed,
  pendingComponent: ScreenLoading,
  errorComponent: RootError,
  // Drawn where the screen would be, so it stands in the frame like any screen.
  notFoundComponent: NotFound,
});

/** The frame the screen in view stands in: what its route says, and otherwise by whether somebody is signed in. */
function useFrame(): RouteFrame {
  const signedIn = useStore(client.session, (state) => state.status === 'signed_in');
  const declared = useMatches({ select: (matches) => matches.at(-1)?.staticData.frame });
  return declared ?? (signedIn ? 'shell' : 'plain');
}

function Framed({ children }: { children: ReactNode }) {
  useScreenCodePrefetch();
  const leavingAdmin = useAdminExit();
  const frame = useFrame();
  if (frame === 'bare') return children;
  return <ShellControls>{frame === 'shell' ? <Suspense fallback={<ScreenLoading />}><Shell leavingAdmin={leavingAdmin}>{children}</Shell></Suspense> : <AuthFrame>{children}</AuthFrame>}</ShellControls>;
}

function Root() {
  return <Outlet />;
}

/** The system could not be asked, or a screen failed in a way nothing nearer caught. */
function RootError({ error }: { error: unknown }) {
  const { t } = useTranslation();
  const router = useRouter();
  const signedIn = useStore(client.session, (state) => state.status === 'signed_in');
  const retry = () => void router.invalidate();

  if (signedIn) {
    return (
      <ErrorState error={error} title={t('shell.screenFailed')} onRetry={retry} className="h-full" />
    );
  }
  return <ConnectionFailure error={error} onRetry={retry} />;
}
