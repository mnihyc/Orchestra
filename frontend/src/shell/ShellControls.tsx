/** The root shell keeps Find available while admission, route code, bootstrap or page data waits.
 *
 * Find's code is fetched when it is first opened, or when the browser is idle after the page has started, whichever is first: the
 * entry holds the key that opens it, not the finder.
 */
import { skipToken, useQuery } from '@tanstack/react-query';
import { type ReactNode, Suspense, useEffect, useState } from 'react';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { lazyPart } from '@/features/files/lazy';
import { useFinder } from '@/features/finder/store';
import type { MeBootstrapReply } from '@/generated/api-types';
import { shellBootstrapKey } from '@/shell/bootstrap';
import { useShellKeyboard } from '@/shell/keyboard';
import { useUnsavedGuard } from '@/features/workbench/store';
import { useChatDraftGuard } from '@/features/chat/draft';
import { useComposerDraftGuard } from '@/features/composer/Draft';

const Finder = lazyPart<{ ready?: boolean }>(() => import('@/features/finder/Finder').then((module) => ({ default: module.Finder })));

/** How long after the page has started the finder's code is fetched in the background, if nobody has opened it by then. */
export const FINDER_IDLE_MS = 3000;

/** True once the finder has been opened, or the page has been standing for `FINDER_IDLE_MS`. */
function useFinderWanted(signedIn: boolean): boolean {
  const opened = useFinder((state) => state.opened > 0);
  const [idle, setIdle] = useState(false);
  useEffect(() => {
    if (!signedIn) return;
    const timer = setTimeout(() => setIdle(true), FINDER_IDLE_MS);
    return () => clearTimeout(timer);
  }, [signedIn]);
  return opened || idle;
}

export function ShellControls({ children }: { children: ReactNode }) {
  const userId = useStore(client.session, (state) => state.userId);
  useShellKeyboard(userId !== null);
  useUnsavedGuard();
  useChatDraftGuard();
  useComposerDraftGuard();
  // Observe the aggregate's readiness; only Shell starts its request.
  const bootstrap = useQuery<MeBootstrapReply>({ queryKey: shellBootstrapKey(userId ?? ''), queryFn: skipToken }).data;
  const finderWanted = useFinderWanted(userId !== null);
  return <>
    {userId !== null && finderWanted && <Suspense fallback={null}><Finder key={userId} ready={bootstrap?.person.id === userId} /></Suspense>}
    {children}
  </>;
}
