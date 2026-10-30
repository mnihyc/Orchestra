/** Shared HelpPanel component (help): the help view's frame: for this screen, search, articles, tutorials, shortcuts.
 *
 * A column that fits a narrow bar as well as a page: a field to search by at its head, a row of what the
 * help has under it, and the one that is chosen below, scrolling by itself. What each holds is the caller's;
 * the frame keeps the tabs reachable by the arrow keys and gives the field and the content their places.
 * While something that fills the panel is open (an article) it stands in place of the tabs.
 */
import type { ReactNode } from 'react';

import { cx } from '@/ui/cx';
import { type TabDefinition, Tabs } from '@/ui/Tabs';
import { scrollAreaClasses } from '@/ui/ScrollArea';

export interface HelpPanelProps<Value extends string> {
  /** The field that searches the help, drawn by the caller. */
  search: ReactNode;
  /** What this panel is, for a screen reader. */
  label: string;
  tabs: readonly TabDefinition<Value>[];
  value: Value;
  onValueChange: (value: Value) => void;
  /** A visible area sidebar already provides these section choices. */
  navigation?: boolean;
  /** Shown instead of the tabs while something that fills the panel is open: an article. */
  open?: ReactNode;
  className?: string;
}

export function HelpPanel<Value extends string>({ search, label, tabs, value, onValueChange, navigation = true, open, className }: HelpPanelProps<Value>) {
  return (
    <section aria-label={label} className={cx('flex h-full min-h-0 flex-col bg-bg', className)}>
      <div className="shrink-0 border-b border-border p-3">{search}</div>
      {open ?? (
        <Tabs
          label={label}
          tabs={tabs}
          value={value}
          onValueChange={onValueChange}
          navigation={navigation}
          className="min-h-0 flex-1"
          panelClassName={cx(scrollAreaClasses, 'flex-1')}
        />
      )}
    </section>
  );
}
