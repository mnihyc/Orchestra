/** Chat addresses reveal their view from the frame, before an unseen chat body is mounted. */
import { useRouterState } from '@tanstack/react-router';
import { useEffect } from 'react';
import { revealView } from '@/features/workbench/openView';
import { currentHeading, useCurrentHeading } from '@/shell/screen';

export function useChatLinks(): void {
  const request = useRouterState({ select: (state) => {
    const search = state.location.search as Record<string, unknown>;
    return { panel: search.panel === 'chat', chat: typeof search.chat === 'string' ? search.chat : '', message: typeof search.message === 'string' ? search.message : '' };
  } });
  const node = useCurrentHeading()?.nodeId;
  useEffect(() => {
    if (!request.panel && request.chat === '' && request.message === '') return;
    // A route boundary can clear the old page between render and this effect.
    if (node === undefined || currentHeading()?.nodeId !== node) return;
    revealView('chat', { node });
  }, [node, request.panel, request.chat, request.message]);
}
