/** Content milestones belong to the actual shown page, after its initial reads and loading shape. */
import { useQueryClient } from '@tanstack/react-query';
import { useContext, useLayoutEffect, type RefObject } from 'react';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { mark } from '@/lib/marks';
import { ViewOpeningContext } from '@/features/workbench/viewOpening';
import { viewReadState } from '@/features/workbench/KeptQueries';

export function usePageContent(address: string | null, view: string, rootRef: RefObject<HTMLElement | null>): void {
  const cache = useQueryClient();
  const opening = useContext(ViewOpeningContext);
  const reader = useStore(client.session, (state) => `${state.status}:${state.userId ?? ''}:${state.readerVersion ?? 0}`);
  useLayoutEffect(() => {
    if (opening === null || address === null) return;
    let published = false;
    let frame = 0;
    let observedRoot: HTMLElement | null = null;
    let stop: () => void = () => undefined;
    const check = () => {
      const root = rootRef.current;
      if (published || root?.isConnected !== true) return;
      const reads = viewReadState(cache);
      if (reads.pending || root.querySelector('[aria-busy="true"], [data-screen-loading]') !== null || root.textContent.trim() === '') return;
      const session = client.session.getState();
      if (session.status !== 'signed_in' || session.userId === null) return;
      published = true;
      const outcome = reads.failed || root.querySelector('[role="alert"]') !== null ? 'error' : 'ready';
      root.dataset.pageAddress = address;
      root.dataset.pageContent = outcome;
      mark('page/content', { user: session.userId, reader: String(session.readerVersion ?? 0), view, address, outcome });
      observer.disconnect(); stop();
    };
    const later = () => { if (!published) { cancelAnimationFrame(frame); frame = requestAnimationFrame(check); } };
    const observer = new MutationObserver(later);
    // Ancestor refs are attached after descendant layout effects; the microtask sees the completed commit.
    queueMicrotask(() => {
      if (published) return;
      const root = rootRef.current;
      if (root === null) return;
      observedRoot = root;
      observer.observe(root, { childList: true, subtree: true, characterData: true, attributes: true });
      check();
    });
    stop = cache.getQueryCache().subscribe(later);
    return () => {
      published = true;
      cancelAnimationFrame(frame); observer.disconnect(); stop();
      const root = observedRoot;
      if (root !== null) { delete root.dataset.pageAddress; delete root.dataset.pageContent; }
    };
  }, [cache, rootRef, opening, view, address, reader]);
}
