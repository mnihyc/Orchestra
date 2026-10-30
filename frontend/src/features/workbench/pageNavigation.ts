/** An addressed pane's URL write retains its instance origin until the router resolves it. */
import { create } from 'zustand';

export interface PageNavigationOrigin { tab: string; href: string }
let origin: PageNavigationOrigin | null = null;
export function markPageNavigation(tab: string, href: string): void { origin = { tab, href }; }
export function pageNavigationOrigin(): PageNavigationOrigin | null { return origin; }
export function clearPageNavigation(held: PageNavigationOrigin | null): void { if (origin === held) origin = null; }
export const usePageFailures = create<{ byTab: ReadonlyMap<string, unknown> }>(() => ({ byTab: new Map() }));
export function setPageFailure(tab: string, error: unknown): void {
  const byTab = new Map(usePageFailures.getState().byTab);
  if (error === undefined) byTab.delete(tab); else byTab.set(tab, error);
  usePageFailures.setState({ byTab });
}
