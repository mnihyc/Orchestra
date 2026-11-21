/** Shared ViewGroup component (workbench): a group of tabs: bringing a view to the front, closing it, splitting the group.
 *
 * A strip of tabs and, under it, the view of the tab in front. The strip is one stop of the Tab key:
 * inside it the arrow keys go from tab to tab, Home and End to its ends, and Enter brings the tab
 * with the keyboard to the front. Complete tabs wrap in the strip; its searchable locator reaches
 * the same visible tabs, including their unsaved marks and Close.
 *
 * The group draws what it is given. What a tab's menu holds, what moves a tab and what its view is
 * are the caller's.
 */
import { Fragment, type KeyboardEvent, type ReactNode } from 'react';

import type { NameKind } from '@/generated/api-types';
import { cx } from '@/ui/cx';
import { useTabStripReveal } from '@/ui/useTabStripReveal';
import { type ChoiceHelp } from '@/ui/Choice';
import { TabLocator } from '@/ui/TabLocator';
import { ScrollArea } from '@/ui/ScrollArea';

export interface GroupTabInfo {
  id: string;
  title: string;
  description?: string;
  path?: string;
  /** A restored page whose name is missing from the saved layout and navigation tree. */
  readonly unnamedTarget?: { kind: NameKind; id: string } | null;
  help?: ChoiceHelp;
  icon?: ReactNode;
  unsaved?: boolean;
  closable?: boolean;
  onSelect: () => void;
  onClose: () => void;
}

export interface ViewGroupProps {
  /** What the group is, for a screen reader: "Right bar". */
  label: string;
  /** The ids of the tabs, in order, and the id of the one in front. */
  tabIds: readonly string[];
  activeId: string | null;
  tabInfo: readonly GroupTabInfo[];
  /** Draws a tab. `tabbable` says whether it is the tab the Tab key reaches. */
  renderTab: (tabId: string, state: { active: boolean; tabbable: boolean }) => ReactNode;
  /** The id the tab element of a tab has, and the id of its panel. */
  tabElementId: (tabId: string) => string;
  panelId: string;
  /** Buttons at the end of the strip that act on the group. */
  actions?: ReactNode;
  /** What stands at the end of the tabs, inside the strip: the place a tab is dropped to join the group last. */
  stripEnd?: ReactNode;
  /** The view of the tab in front. */
  children: ReactNode;
  /** What is shown while the group holds no tab. */
  empty?: ReactNode;
  /** Marks the group that has the keyboard. */
  focused?: boolean;
  /** A grid cell with one tab needs no locator for that sole tab. */
  compactTabs?: boolean;
  /** Something drawn over the view: the places a carried tab can be dropped. */
  overlay?: ReactNode;
  onFocusWithin?: () => void;
  className?: string;
}

export function ViewGroup({
  label,
  tabIds,
  activeId,
  tabInfo,
  renderTab,
  tabElementId,
  panelId,
  actions,
  stripEnd,
  children,
  empty,
  focused = false,
  compactTabs = false,
  overlay,
  onFocusWithin,
  className,
}: ViewGroupProps) {
  const strip = useTabStripReveal(activeId, tabIds, tabElementId, panelId);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey) return;
    const target = event.target as HTMLElement;
    if (target.getAttribute('role') !== 'tab') return;
    const index = tabIds.findIndex((id) => tabElementId(id) === target.id);
    const go = (to: number) => {
      const id = tabIds[(to + tabIds.length) % tabIds.length];
      if (id === undefined) return;
      event.preventDefault();
      tabInfo.find((tab) => tab.id === id)?.onSelect();
      requestAnimationFrame(() => document.getElementById(tabElementId(id))?.focus());
    };
    if (event.key === 'ArrowRight') go(index + 1);
    else if (event.key === 'ArrowLeft') go(index - 1);
    else if (event.key === 'Home') go(0);
    else if (event.key === 'End') go(tabIds.length - 1);
  };

  // The tab the Tab key reaches: the one in front, or the first when none is.
  const tabbableId = activeId ?? tabIds[0] ?? null;

  return (
    <section
      aria-label={label}
      data-view-group
      className={cx('flex h-full min-h-0 min-w-0 flex-col bg-bg', className)}
      onFocusCapture={onFocusWithin}
      onPointerDownCapture={onFocusWithin}
    >
      <div
        className={cx(
          'flex min-h-(--tab-strip-height) shrink-0 items-stretch border-b border-border bg-bg-subtle',
          focused && 'shadow-[inset_0_-1px_0_var(--accent)]',
        )}
      >
        <div className="relative flex min-w-0 flex-1 items-stretch" data-navigation-strip="views">
        <div
          ref={strip}
          role="tablist"
          aria-label={label}
          aria-orientation="horizontal"
          className="flex max-h-[calc(var(--tab-strip-height)*2)] min-w-0 flex-1 flex-wrap items-stretch overflow-y-auto snap-y snap-mandatory"
          onKeyDown={onKeyDown}
        >
          {tabIds.map((tabId) => <Fragment key={tabId}>{renderTab(tabId, { active: tabId === activeId, tabbable: tabId === tabbableId })}</Fragment>)}
          {stripEnd}
        </div>
        {(!compactTabs || tabIds.length > 1) && <span className="flex h-(--tab-strip-height) shrink-0 items-center self-start">
          <TabLocator label={label} activeId={activeId} tabs={tabInfo} tabElementId={tabElementId} />
        </span>}
        </div>
        {actions !== undefined && <div className="flex h-(--tab-strip-height) shrink-0 items-center gap-0.5 px-1">{actions}</div>}
      </div>
      <div className="relative min-h-0 flex-1">
        {activeId === null ? (
          <ScrollArea className="h-full">{empty}</ScrollArea>
        ) : (
          <ScrollArea
            role="tabpanel"
            id={panelId}
            aria-labelledby={tabElementId(activeId)}
            tabIndex={-1}
            className="h-full outline-none"
          >
            <section aria-label={tabInfo.find((tab) => tab.id === activeId)?.title} className="h-full min-h-0">{children}</section>
          </ScrollArea>
        )}
        {overlay}
      </div>
    </section>
  );
}
