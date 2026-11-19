/** Only resolved browser navigation enters the workspace. A pane's URL echo keeps its exact origin. */
import { useMatches, useRouter, useRouterState } from '@tanstack/react-router';
import { useLayoutEffect, useReducer, useRef } from 'react';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { usePageTitle } from '@/lib/title';
import { focusScreenHeading, useCurrentHeading } from '@/shell/screen';
import { allTabs, findTab, mapGroups, setTabTarget } from './layout';
import { EMPTY_WORKBENCH, canonicalPageAddress, openPage, pageAddress } from './PageView';
import { clearPageNavigation, markPageNavigation, pageNavigationOrigin, setPageFailure } from './pageNavigation';
import { useWorkbench } from './store';
import { bindContentNavigation } from './contentNavigation';
import { contentTab, parseContentAddress } from './contentAddress';

export function PageRouting({ ready, onAddressed }: { ready: boolean; onAddressed?: () => void }) {
  const user = useStore(client.session, (state) => state.userId);
  const router = useRouter();
  const location = useRouterState({ select: (state) => state.location });
  const settled = useRouterState({ select: (state) => !state.isLoading && state.status === 'idle' });
  const rejected = useMatches({ select: (matches) => matches.find((match) => match.status === 'error' || match.status === 'notFound')?.error });
  const screen = useMatches({ select: (matches) => matches.at(-1)?.staticData.screen }) ?? 'page';
  const active = useWorkbench((state) => state.activeContent);
  const layout = useWorkbench((state) => state.layout);
  const last = useRef<string | null>(null);
  const resolving = useRef<string | null>(null);
  const [resolvedVisit, reconsiderFocus] = useReducer((version: number) => version + 1, 0);
  const heading = useCurrentHeading();
  usePageTitle(heading?.title);
  useLayoutEffect(() => focusScreenHeading(active), [active]);
  useLayoutEffect(() => bindContentNavigation((owner, href, replace) => {
    const state = useWorkbench.getState(), tab = findTab(state.layout, owner)?.tab;
    if (tab === undefined || state.userId !== client.session.getState().userId || pageAddress(tab) !== href) return;
    markPageNavigation(owner, href);
    void router.navigate({ href, replace });
  }), [router]);

  useLayoutEffect(() => {
    if (!ready || user === null || !settled) return;
    useWorkbench.getState().enter(user, screen);
    const state = useWorkbench.getState();
    const href = location.href;
    const publishFocus = () => {
      if (router.state.isLoading || router.state.status !== 'idle') return;
      const current = useWorkbench.getState();
      const tab = current.activeContent === null ? null : findTab(current.layout, current.activeContent)?.tab;
      const address = tab === null || tab === undefined ? EMPTY_WORKBENCH : pageAddress(tab) ?? EMPTY_WORKBENCH;
      const browser = router.state.location.href;
      if (address !== browser && `${address}/` !== browser) {
        if (tab !== null && tab !== undefined) markPageNavigation(tab.id, address);
        void router.navigate({ href: address, replace: true });
      }
    };
    const visit = `${location.state.__TSR_key ?? ''}:${href}`;
    if (resolving.current === visit) return;
    if (last.current !== visit) {
      const first = last.current === null;
      last.current = visit;
      const enterAddress = () => {
        const fresh = useWorkbench.getState();
        if (href === EMPTY_WORKBENCH || href === `${EMPTY_WORKBENCH}/`) return;
        const current = fresh.activeContent === null ? null : findTab(fresh.layout, fresh.activeContent);
        if (first && current !== null && pageAddress(current.tab) === href) { setPageFailure(current.tab.id, rejected); return; }
        const legacy = allTabs(fresh.layout).find((tab) => tab.view === 'screen');
        const requested = parseContentAddress(href);
        const resource = requested === null ? { view: 'page', of: { href } } : contentTab(requested);
        let id: string | null;
        if (legacy !== undefined) {
          fresh.change((value) => mapGroups(value, (group) => ({ ...group, tabs: group.tabs.map((tab) => tab.id === legacy.id ? { ...tab, ...resource } : tab) })));
          fresh.focus(legacy.id); id = legacy.id;
        } else id = requested === null ? fresh.openTab(resource.view, resource.of) : openPage(href);
        if (id !== null) setPageFailure(id, rejected);
      };
      const restored = first && state.activeContent !== null ? findTab(state.layout, state.activeContent)?.tab : undefined;
      const restoredHref = restored === undefined ? null : pageAddress(restored);
      if (pageNavigationOrigin() === null && restored !== undefined && restoredHref !== null && restoredHref !== href) markPageNavigation(restored.id, restoredHref);
      const origin = pageNavigationOrigin();
      const owned = origin === null ? null : findTab(state.layout, origin.tab);
      if (origin !== null && owned === null) {
        // Closing an owner while its navigation is pending cannot recreate its presentation.
        clearPageNavigation(origin);
        const focused = state.activeContent === null ? null : findTab(state.layout, state.activeContent)?.tab;
        const onward = focused === null || focused === undefined ? EMPTY_WORKBENCH : pageAddress(focused) ?? EMPTY_WORKBENCH;
        if (onward !== href) {
          if (focused !== null && focused !== undefined) markPageNavigation(focused.id, onward);
          void router.navigate({ href: onward, replace: true });
        }
        return;
      }
      if (origin?.href === href && owned !== null && pageAddress(owned.tab) === href) {
        clearPageNavigation(origin); setPageFailure(owned.tab.id, rejected);
        // This is an acknowledgement of the earlier focus, which may already have changed.
        publishFocus(); return;
      }
      if (origin !== null && owned !== null && origin.href !== href) {
        // A canonical loader result belongs to the same instance that requested its alias.
        // The captured visit and origin fence late resolutions after another navigation.
        resolving.current = visit;
        void canonicalPageAddress(origin.href).then((canonical) => {
          if (last.current !== visit || pageNavigationOrigin() !== origin) return;
          const fresh = useWorkbench.getState(), held = findTab(fresh.layout, origin.tab);
          if (held === null) return;
          if (canonical === href && [origin.href, href].includes(pageAddress(held.tab) ?? '')) {
            const next = parseContentAddress(href);
            if (next !== null) fresh.change((value) => setTabTarget(value, origin.tab, contentTab(next).of));
            setPageFailure(origin.tab, rejected);
          } else enterAddress();
        }, () => { if (last.current === visit && pageNavigationOrigin() === origin) enterAddress(); }).finally(() => {
          clearPageNavigation(origin);
          if (resolving.current === visit) {
            resolving.current = null;
            // Focus can change while the old alias lookup is held. Settling that lookup must
            // revisit the current pane's URL even when it caused no other React state change.
            reconsiderFocus();
          }
        });
        return;
      }
      clearPageNavigation(origin);
      if (href !== EMPTY_WORKBENCH && href !== `${EMPTY_WORKBENCH}/`) { enterAddress(); return; }
    }
    publishFocus();
  }, [ready, user, location, settled, rejected, active, layout, router, screen, resolvedVisit]);
  useLayoutEffect(() => {
    if (!ready || user === null || !settled) return;
    const state = useWorkbench.getState();
    const tab = state.activeContent === null ? null : findTab(state.layout, state.activeContent)?.tab;
    const address = tab === null || tab === undefined ? EMPTY_WORKBENCH : pageAddress(tab) ?? EMPTY_WORKBENCH;
    if (location.href === EMPTY_WORKBENCH || location.href === `${EMPTY_WORKBENCH}/`
      || address === location.href || `${address}/` === location.href) onAddressed?.();
  }, [ready, user, settled, location.href, active, layout, onAddressed]);
  return null;
}
