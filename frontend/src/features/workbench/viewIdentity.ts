/** A view keeps its identity when its section, group or arrangement changes. */
import type { Tab } from './layout';

export function viewKey(tab: Tab): string {
  return tab.id;
}
