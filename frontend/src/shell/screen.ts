/** What the screen in view says of itself: its name, the node it is the screen of, and the section it belongs to.
 *
 * The workbench draws the title of the main area, the tab of the screen and the title of the
 * browser's tab, and knows no screen. So each screen says these few things about itself while it is
 * in view, and the workbench reads them here.
 */
import type { ThreadIconFacts } from '@/ui/ThreadState';
import type { LucideIcon } from 'lucide-react';
import type { NodeRef, PersonalTarget } from '@/generated/api-types';
import { useContext, useEffect, useMemo, useRef } from 'react';
import { create } from 'zustand';
import { useShallow } from 'zustand/react/shallow';

import { useNodeLabel } from '@/features/navigation/placeLabels';
import { usePageTitle } from '@/lib/title';
import { ViewOpeningContext } from '@/features/workbench/viewOpening';
import { useViewInstance } from '@/features/workbench/instance';

/** One place above the screen in view, as a way to it: what it is called and where it leads. */
export interface TrailPart {
  label: string;
  /** The route it leads to, as the router writes it (`/resources/agents`). */
  to: string;
  /** What the address of that route says, such as the filter a list was last left with. */
  search?: Readonly<Record<string, string>> | undefined;
}

export interface ScreenHeading {
  /** Reader-specific thread facts for the existing heading and tab icon slots. */
  thread?: ThreadIconFacts | undefined;
  /** The addressed thing whose screen this is; its kind/id is also the personal visit identity. */
  target?: PersonalTarget | undefined;
  /** The name of the screen, or of the thing it shows. */
  title: string;
  /** The view's tab label, including readable place context for a subpage. */
  tabTitle?: string | undefined;
  /** Keep an already read tab identity while this view's own metadata is pending. */
  tabPending?: boolean | undefined;
  /** An API-authorized path can include redacted places absent from the live navigation tree. */
  path?: readonly NodeRef[] | undefined;
  /** This route is a thing or a sub-screen below the named node. */
  leaf?: boolean | undefined;
  /** Only contextual views that can actually be read or filled are offered. */
  views?: { ready: boolean; chat?: boolean; workspace?: boolean; terminal?: boolean; processes?: boolean; help?: boolean; links?: boolean } | undefined;
  icon?: LucideIcon | undefined;
  /** The node the screen is of: the path above it is then drawn from the tree. */
  nodeId?: string | undefined;
  /** For a screen that is one of several under a common name: that name ("Settings", "Resources"). */
  section?: string | undefined;
  /**
   * The places above the screen, outermost first, each a way to it ("Resources", "Agents" above a profile).
   * Where it is given it takes the place of the section as the way to the screen, so that
   * every part but the last is a link, and the first of them back to the list the screen was reached from.
   */
  trail?: readonly TrailPart[] | undefined;
}

interface Headings { heading: ScreenHeading | null; route: ScreenHeading | null }
/** Native identity owns readiness and names; its matching admitted body may supply the resource trail. */
function resolvedHeading(held: Headings | null | undefined): ScreenHeading | null {
  const route = held?.route, body = held?.heading;
  if (route === null || route === undefined) return body ?? null;
  if (route.views?.ready === false || body?.target === undefined) return route;
  if (route.target?.kind !== body.target.kind || route.target.id !== body.target.id) return route;
  const section = route.section ?? body.section, trail = route.trail ?? body.trail;
  return section === route.section && trail === route.trail ? route : { ...route, section, trail };
}
const useHeading = create<Headings & { focused: string | null; byView: ReadonlyMap<string, Headings> }>(() => ({ heading: null, route: null, focused: null, byView: new Map() }));
function report(owner: string | undefined, kind: keyof Headings, heading: ScreenHeading | null): void {
  const before = useHeading.getState();
  if (owner === undefined) { if (before[kind] !== heading) useHeading.setState({ [kind]: heading }); return; }
  const held = before.byView.get(owner) ?? { heading: null, route: null };
  if (held[kind] === heading) return;
  const byView = new Map(before.byView);
  byView.set(owner, { ...held, [kind]: heading });
  useHeading.setState({ byView });
}
export function focusScreenHeading(focused: string | null): void { useHeading.setState({ focused }); }
export function forgetScreenHeading(owner: string): void {
  const byView = new Map(useHeading.getState().byView);
  if (byView.delete(owner)) useHeading.setState({ byView });
}


/** Replacing the current heading publishes one update. Only leaving its lifetime clears a global heading. */
function usePublishedHeading(owner: string | undefined, kind: keyof Headings, value: ScreenHeading, enabled: boolean, clearOnLeave: boolean) {
  const published = useRef<ScreenHeading | null>(null);
  useEffect(() => {
    if (!enabled) return;
    published.current = value;
    report(owner, kind, value);
  }, [owner, kind, value, enabled, clearOnLeave]);
  useEffect(() => {
    if (!enabled || !clearOnLeave) return;
    return () => {
      if (useHeading.getState()[kind] === published.current) report(undefined, kind, null);
    };
  }, [enabled, clearOnLeave, kind, owner]);
}

/** The authorized path stays the same input for navigation when only a thread's display changes. */
function useReportedPath(path: ScreenHeading['path']): ScreenHeading['path'] {
  const encoded = path === undefined ? '' : JSON.stringify(path);
  return useMemo(() => encoded === '' ? undefined : JSON.parse(encoded) as readonly NodeRef[], [encoded]);
}

/** Addressed resource identity belongs to the frame even when its Screen body is paused. */
export function useRouteHeading(heading: ScreenHeading): void {
  const instance = useViewInstance();
  const owner = instance?.id;
  const { title, tabTitle, tabPending, icon, nodeId, leaf, target, section, trail } = heading;
  const presented = JSON.stringify(heading.thread ?? null);
  const path = useReportedPath(heading.path);
  const views = JSON.stringify(heading.views ?? null);
  const ancestry = trail === undefined ? '' : JSON.stringify(trail);
  const targetKind = target?.kind, targetId = target?.id;
  usePageTitle(useNodeLabel(nodeId, title), instance === null);
  const route = useMemo<ScreenHeading>(() => ({ thread: presented === 'null' ? undefined : JSON.parse(presented) as ThreadIconFacts, title, tabTitle, icon, nodeId, leaf, section,
      trail: ancestry === '' ? undefined : JSON.parse(ancestry) as readonly TrailPart[], ...(tabPending === undefined ? {} : { tabPending }),
      path, views: views === 'null' ? undefined : JSON.parse(views) as ScreenHeading['views'],
      target: targetKind === undefined || targetId === undefined ? undefined : { kind: targetKind, id: targetId } }),
    [title, tabTitle, tabPending, icon, nodeId, leaf, section, ancestry, presented, path, views, targetKind, targetId]);
  usePublishedHeading(owner, 'route', route, true, owner === undefined);
}

/** Says what the screen in view is, for as long as the calling component is in view. */
export function useScreenHeading(heading: ScreenHeading, enabled = true): void {
  const inWorkbench = useContext(ViewOpeningContext) !== undefined;
  const instance = useViewInstance();
  const owner = instance?.id;
  const { title, tabTitle, tabPending, icon, nodeId, section, trail, leaf, views, path, target } = heading;
  const targetKind = target?.kind;
  const targetId = target?.id;
  // The browser's tab names a place with where it is when another place the reader sees has its name.
  const ownsBrowserTitle = enabled && instance === null;
  usePageTitle(useNodeLabel(ownsBrowserTitle ? nodeId : null, title), ownsBrowserTitle);
  // A trail is made again by every draw of the screen; what it says is what counts, so it is compared by that.
  const said = trail === undefined ? '' : JSON.stringify(trail);
  const availability = views === undefined ? '' : JSON.stringify(views);
  const authorized = useReportedPath(path);
  const presented = JSON.stringify(heading.thread ?? null);
  const spoken = useMemo<ScreenHeading>(() => ({ thread: presented === 'null' ? undefined : JSON.parse(presented) as ThreadIconFacts, title, tabTitle, icon, nodeId, section, leaf, ...(tabPending === undefined ? {} : { tabPending }), target: targetKind === undefined || targetId === undefined ? undefined : { kind: targetKind, id: targetId }, path: authorized, views: availability === '' ? undefined : JSON.parse(availability) as ScreenHeading['views'], trail: said === '' ? undefined : (JSON.parse(said) as TrailPart[]) }),
    [title, tabTitle, tabPending, icon, nodeId, section, said, leaf, availability, authorized, presented, targetKind, targetId]);
  usePublishedHeading(owner, 'heading', spoken, enabled, !inWorkbench);
}

/** What the screen in view said of itself, or `null` while it has said nothing. */
export function useCurrentHeading(enabled?: boolean): ScreenHeading | null;
export function useCurrentHeading<T>(enabled: boolean, select: (heading: ScreenHeading) => T): T | null;
export function useCurrentHeading<T>(enabled = true, select?: (heading: ScreenHeading) => T) {
  const instance = useViewInstance();
  return useHeading(useShallow((state) => {
    if (!enabled) return null;
    const owner = instance === null ? state.focused : instance.headingOwner;
    const held = owner === null ? (instance === null ? state : null) : state.byView.get(owner);
    const heading = resolvedHeading(held);
    return select === undefined || heading === null ? heading : select(heading);
  }));
}

/** The current availability for non-React commands and view offers. */
export function currentHeading(): ScreenHeading | null { const state = useHeading.getState(); const held = state.focused === null ? state : state.byView.get(state.focused); return resolvedHeading(held); }

/** A new addressed page must not inherit the previous page's names or availability. */
export function clearScreenHeading(): void { useHeading.setState({ heading: null, route: null }); }
