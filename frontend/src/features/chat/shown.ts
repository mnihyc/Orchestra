/** Whether a view of the workbench is in front of its reader: its tab is the one showing in its group, and its area is open.
 *
 * Every view of the layout is kept drawn, in front or not, so that moving a tab never loses what a view
 * held. A view that follows something live asks here whether it is seen, and follows it only then. A view
 * drawn as a sheet on a small screen has no tab in the layout; it is shown for as long as the sheet is.
 */
import type { AreaId, Tab } from '@/features/workbench/layout';
import { useViewShown as useShownLease } from '@/features/workbench/viewOpening';

export function useViewShown(_tab: Tab, _area: AreaId): boolean {
  return useShownLease();
}
