/** What the screen in view says of itself: its name, the node it is the screen of, and the section it belongs to.
 *
 * The workbench draws the title of the main area, the tab of the screen and the title of the
 * browser's tab, and knows no screen. So each screen says these few things about itself while it is
 * in view, and the workbench reads them here.
 */
import type { ThreadIconFacts } from '@/ui/ThreadState';
import type { LucideIcon } from 'lucide-react';
import type { NodeRef, PersonalTarget } from '@/generated/api-types';
import { useContext, useEffect } from 'react';
import { create } from 'zustand';

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
const useHeading = create<Headings & { focused: string | null; byView: ReadonlyMap<string, Headings> }>(() => ({ heading: null, route: null, focused: null, byView: new Map() }));
function report(owner: string | undefined, kind: keyof Headings, heading: ScreenHeading | null): void {
  if (owner === undefined) { useHeading.setState({ [kind]: heading }); return; }
  const before = useHeading.getState();
  const held = before.byView.get(owner) ?? { heading: null, route: null };
  const byView = new Map(before.byView);
  byView.set(owner, { ...held, [kind]: heading });
  useHeading.setState({ byView });
}
export function focusScreenHeading(focused: string | null): void { useHeading.setState({ focused }); }
export function forgetScreenHeading(owner: string): void {
  const byView = new Map(useHeading.getState().byView);
  if (byView.delete(owner)) useHeading.setState({ byView });
}


/** Addressed resource identity belongs to the frame even when its Screen body is paused. */
export function useRouteHeading(heading: ScreenHeading): void {
  const instance = useViewInstance();
  const owner = instance?.id;
  const { title, tabTitle, tabPending, icon, nodeId, leaf, target } = heading;
  const presented = JSON.stringify(heading.thread ?? null);
  const path = JSON.stringify(heading.path ?? null);
  const views = JSON.stringify(heading.views ?? null);
  const targetKind = target?.kind, targetId = target?.id;
  usePageTitle(useNodeLabel(nodeId, title), instance === null);
  useEffect(() => {
    const route: ScreenHeading = { thread: presented === 'null' ? undefined : JSON.parse(presented) as ThreadIconFacts, title, tabTitle, icon, nodeId, leaf, ...(tabPending === undefined ? {} : { tabPending }),
      path: path === 'null' ? undefined : JSON.parse(path) as ScreenHeading['path'], views: views === 'null' ? undefined : JSON.parse(views) as ScreenHeading['views'],
      target: targetKind === undefined || targetId === undefined ? undefined : { kind: targetKind, id: targetId } };
    report(owner, 'route', route);
    return () => { if (owner === undefined && useHeading.getState().route === route) report(owner, 'route', null); };
  }, [title, tabTitle, tabPending, icon, nodeId, leaf, presented, path, views, targetKind, targetId, owner]);
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
  usePageTitle(useNodeLabel(nodeId, title), enabled && instance === null);
  // A trail is made again by every draw of the screen; what it says is what counts, so it is compared by that.
  const said = trail === undefined ? '' : JSON.stringify(trail);
  const availability = views === undefined ? '' : JSON.stringify(views);
  const authorized = path === undefined ? '' : JSON.stringify(path);
  const presented = JSON.stringify(heading.thread ?? null);
  useEffect(() => {
    if (!enabled) return;
    const spoken: ScreenHeading = { thread: presented === 'null' ? undefined : JSON.parse(presented) as ThreadIconFacts, title, tabTitle, icon, nodeId, section, leaf, ...(tabPending === undefined ? {} : { tabPending }), target: targetKind === undefined || targetId === undefined ? undefined : { kind: targetKind, id: targetId }, path: authorized === '' ? undefined : JSON.parse(authorized) as readonly NodeRef[], views: availability === '' ? undefined : JSON.parse(availability) as ScreenHeading['views'], trail: said === '' ? undefined : (JSON.parse(said) as TrailPart[]) };
    report(owner, 'heading', spoken);
    return () => {
      // Activity pauses hidden page effects. Its already-read identity still belongs to the frame;
      // the shell clears it when the addressed route changes or the frame is left.
      if (inWorkbench) return;
      // Only what this screen said is taken back: the next screen may have spoken already.
      if (useHeading.getState().heading === spoken) useHeading.setState({ heading: null });
    };
  }, [title, tabTitle, tabPending, icon, nodeId, section, said, leaf, availability, authorized, presented, enabled, targetKind, targetId, inWorkbench, owner]);
}

/** What the screen in view said of itself, or `null` while it has said nothing. */
export function useCurrentHeading(): ScreenHeading | null {
  const instance = useViewInstance();
  return useHeading((state) => {
    const owner = instance === null ? state.focused : instance.headingOwner;
    const held = owner === null ? (instance === null ? state : null) : state.byView.get(owner);
    return held?.route ?? held?.heading ?? null;
  });
}

/** The current availability for non-React commands and view offers. */
export function currentHeading(): ScreenHeading | null { const state = useHeading.getState(); const held = state.focused === null ? state : state.byView.get(state.focused); return held?.route ?? held?.heading ?? null; }

/** A new addressed page must not inherit the previous page's names or availability. */
export function clearScreenHeading(): void { useHeading.setState({ heading: null, route: null }); }
