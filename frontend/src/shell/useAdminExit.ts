/** Ending native administrator reach leaves an inaccessible page through its nearest ordinary ancestor. */
import { useQueryClient } from '@tanstack/react-query';
import { useRouter, useRouterState } from '@tanstack/react-router';
import { useEffect, useRef, useState } from 'react';
import { useStore } from 'zustand';
import { adminModeKey } from '@/api/adminMode';
import { api, client } from '@/api/client';
import type { AdminMode } from '@/generated/api-types';
import { useTreeStore } from '@/realtime/stores/tree';
import { useThreadStore } from '@/realtime/stores/thread';
import { useCurrentHeading } from '@/shell/screen';
import { mainContent } from '@/shell/areas';
import { adminExitPlan, ordinaryDestination, type AdminExitPlan, type ExitPlace } from './adminExit';

/** Exact reads use the ordinary session after the native mode answer; no role is guessed locally. */
async function probe(place: ExitPlace, signal: AbortSignal): Promise<boolean> {
  if (place.id === undefined) return false;
  try {
    const response = place.kind === 'doc'
      ? await api.GET('/documents/{id}', { params: { path: { id: place.id } }, signal })
      : place.kind === 'org'
        ? await api.GET('/orgs/by-slug/{slug}', { params: { path: { slug: place.id } }, signal })
        : await api.GET('/nodes/{id}', { params: { path: { id: place.id } }, signal });
    return response.response.ok;
  } catch { return false; }
}

export function useAdminExit(): boolean {
  const cache = useQueryClient();
  const router = useRouter();
  const href = useRouterState({ select: (state) => state.location.href });
  const person = useStore(client.session, (state) => state.userId);
  const heading = useCurrentHeading();
  const kept = useRef<{ person: string | null; ordinary?: string; plan?: AdminExitPlan }>({ person: null });
  const [pendingAt, setPendingAt] = useState<string | null>(null);
  const opening = `${person ?? ''}:${href}`;

  useEffect(() => {
    if (kept.current.person !== person) kept.current = { person };
    const plan = adminExitPlan(cache, href, kept.current.ordinary);
    if (plan.person !== person || person === null) return;
    if (plan.current?.access !== 'unknown' || kept.current.plan?.origin !== href) kept.current.plan = plan;
    if ((plan.current?.access ?? 'yes') === 'yes') kept.current.ordinary = href;
  }, [cache, href, person, heading]);

  useEffect(() => {
    if (kept.current.person !== person) kept.current = { person };
    let wasOn = cache.getQueryData<AdminMode>(adminModeKey)?.as_admin;
    let controller: AbortController | null = null;
    const capture = () => {
      const plan = adminExitPlan(cache, href, kept.current.ordinary);
      // A cache being removed during the authority transition must not erase the last native path.
      if (plan.person !== person || person === null) return;
      if (plan.current?.access !== 'unknown' || kept.current.plan?.origin !== href) kept.current.plan = plan;
      if ((plan.current?.access ?? 'yes') === 'yes') kept.current.ordinary = href;
    };
    const leave = () => {
      if (person === null || controller !== null) return;
      capture();
      const plan = kept.current.plan;
      if (plan?.person !== person || plan.origin !== href || plan.current === null || plan.current.access === 'yes') return;
      controller = new AbortController();
      const active = controller;
      setPendingAt(opening);
      void ordinaryDestination(plan, (place) => probe(place, active.signal)).then(async (target) => {
        if (active.signal.aborted || client.session.getState().userId !== person || router.state.location.href !== href) return;
        if (target !== null && target !== href) {
          // A lost View grant cannot leave the old page standing behind an unsaved-text blocker.
          await router.navigate({ href: target, replace: true, ignoreBlocker: true });
          requestAnimationFrame(() => mainContent()?.focus({ preventScroll: true }));
        }
      }).catch(() => undefined).finally(() => { if (!active.signal.aborted) { controller = null; setPendingAt(null); } });
    };
    capture();
    const observe = () => {
      const mode = cache.getQueryData<AdminMode>(adminModeKey)?.as_admin;
      if (mode === undefined) return;
      if (wasOn === true && !mode) { wasOn = false; leave(); }
      else { wasOn = mode; if (mode) capture(); }
    };
    const offAct = client.onAct((act) => {
      if (act.method !== 'PUT' || act.path !== '/me/admin-mode' || typeof act.answer !== 'object' || act.answer === null || !('as_admin' in act.answer) || typeof act.answer.as_admin !== 'boolean') return;
      if (act.answer.as_admin) { wasOn = true; capture(); }
      else if (wasOn === true) { wasOn = false; leave(); }
    });
    const stopCache = cache.getQueryCache().subscribe(observe);
    const stopTree = useTreeStore.subscribe(() => { if (wasOn === true) capture(); });
    const stopThreads = useThreadStore.subscribe(() => { if (wasOn === true) capture(); });
    return () => { offAct(); stopCache(); stopTree(); stopThreads(); controller?.abort(); };
  }, [cache, router, href, person, opening]);

  // A committed destination has its own normal view; a transient check never alters a saved layout.
  return pendingAt === opening;
}
