/** The root shell keeps Find available while admission, route code, bootstrap or page data waits. */
import { skipToken, useQuery } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { Finder } from '@/features/finder/Finder';
import type { MeBootstrapReply } from '@/generated/api-types';
import { shellBootstrapKey } from '@/shell/bootstrap';
import { useShellKeyboard } from '@/shell/keyboard';
import { useUnsavedGuard } from '@/features/workbench/store';
import { useChatDraftGuard } from '@/features/chat/draft';

export function ShellControls({ children }: { children: ReactNode }) {
  const userId = useStore(client.session, (state) => state.userId);
  useShellKeyboard(userId !== null);
  useUnsavedGuard();
  useChatDraftGuard();
  // Observe the aggregate's readiness; only Shell starts its request.
  const bootstrap = useQuery<MeBootstrapReply>({ queryKey: shellBootstrapKey(userId ?? ''), queryFn: skipToken }).data;
  return <>
    {userId !== null && <Finder key={userId} ready={bootstrap?.person.id === userId} />}
    {children}
  </>;
}
