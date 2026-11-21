/** The root shell keeps Find available while admission, route code, bootstrap or page data waits.
 *
 * Find's code is fetched when it is first opened, or when the browser is idle after first content, whichever is first: the
 * entry holds the key that opens it, not the finder.
 */
import { skipToken, useQuery } from '@tanstack/react-query';
import { type ReactNode, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { lazyPart } from '@/features/files/lazy';
import { closeFinder, useFinder } from '@/features/finder/store';
import { DeferredDialog } from '@/ui/DeferredDialog';
import type { MeBootstrapReply } from '@/generated/api-types';
import { shellBootstrapKey } from '@/shell/bootstrap';
import { useShellKeyboard } from '@/shell/keyboard';
import { useUnsavedGuard } from '@/features/workbench/store';
import { useChatDraftGuard } from '@/features/chat/draft';
import { useComposerDraftGuard } from '@/features/composer/Draft';
import { whenContentShown } from '@/lib/marks';

const Finder = lazyPart<{ ready?: boolean }>(() => import('@/features/finder/Finder').then((module) => ({ default: module.Finder })));

/** How long after first content the finder's code becomes eligible for an idle fetch, unless opened sooner. */
export const FINDER_IDLE_MS = 3000;

/** An explicit opening is immediate; speculative code belongs after this reader's content and an idle turn. */
function useFinderWanted(userId: string | null, readerVersion: number | undefined): boolean {
  const opened = useFinder((state) => state.opened > 0);
  const key = userId === null ? null : JSON.stringify([userId, readerVersion ?? 0]);
  const [idleFor, setIdleFor] = useState<string | null>(null);
  useEffect(() => {
    if (userId === null) return;
    const away = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined, idle: number | undefined;
    void whenContentShown(userId, readerVersion, away.signal).then((shown) => {
      if (!shown || away.signal.aborted) return;
      timer = setTimeout(() => {
        const ready = () => { if (!away.signal.aborted) setIdleFor(key); };
        if (typeof requestIdleCallback === 'function') idle = requestIdleCallback(ready);
        else ready();
      }, FINDER_IDLE_MS);
    });
    return () => {
      away.abort(); clearTimeout(timer);
      if (idle !== undefined && typeof cancelIdleCallback === 'function') cancelIdleCallback(idle);
    };
  }, [userId, readerVersion, key]);
  return opened || key !== null && idleFor === key;
}

export function ShellControls({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const userId = useStore(client.session, (state) => state.userId);
  const readerVersion = useStore(client.session, (state) => state.readerVersion);
  useShellKeyboard(userId !== null);
  useUnsavedGuard();
  useChatDraftGuard();
  useComposerDraftGuard();
  // Observe the aggregate's readiness; only Shell starts its request.
  const bootstrap = useQuery<MeBootstrapReply>({ queryKey: shellBootstrapKey(userId ?? ''), queryFn: skipToken }).data;
  const finderWanted = useFinderWanted(userId, readerVersion);
  const finderOpen = useFinder((state) => state.open);
  return <>
    {userId !== null && finderWanted && <DeferredDialog open={finderOpen} onClose={closeFinder} title={t('finder.title')} resetKey={userId}><Finder key={userId} ready={bootstrap?.person.id === userId} /></DeferredDialog>}
    {children}
  </>;
}
