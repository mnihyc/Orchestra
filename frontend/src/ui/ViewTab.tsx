/** Shared ViewTab component (workbench): a tab with its menu, its dot for something unsaved, and the handle it is moved by.
 *
 * A tab names a view: its icon and its title, a dot while it holds something unsaved, a pin while it
 * is pinned, and a cross that closes it when unpinned. Its menu opens under the right mouse
 * button, with the menu key and with Shift+F10. The whole tab is the handle it is moved by; what
 * moves it is given from outside, so that a tab knows nothing of where it can go.
 */
import { Ellipsis, Pin, X } from 'lucide-react';
import type { HTMLAttributes, ReactNode, Ref } from 'react';
import { useTranslation } from 'react-i18next';

import { ContextMenu } from '@/ui/ContextMenu';
import { cx } from '@/ui/cx';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { Tooltip } from '@/ui/Tooltip';

export interface ViewTabProps {
  /** The id of the tab element; its panel names it. */
  id: string;
  /** The id of the panel the tab shows. */
  panelId: string;
  title: string;
  icon: ReactNode;
  /** One sentence that says what the view shows, read when the pointer rests on the tab. */
  about?: string | undefined;
  active: boolean;
  /** Whether this is the tab of its group that the Tab key reaches. */
  tabbable: boolean;
  /** Holds something unsaved. */
  unsaved?: boolean;
  /** Stays through navigation. */
  kept?: boolean;
  pinned?: boolean;
  closable?: boolean;
  /** Uses narrower padding and a small unsaved mark when the strip has little room. */
  compact?: boolean;
  /** A mark at the tab's leading edge that says a carried tab would land before it. */
  dropBefore?: boolean;
  /** The tab is being carried: it is drawn faint where it came from. */
  carried?: boolean;
  menu: readonly MenuEntry[];
  onSelect: () => void;
  onClose: () => void;
  onDoubleClick?: (() => void) | undefined;
  /** What makes the tab a handle: the ref and the attributes of whatever moves it. */
  handleRef?: Ref<HTMLDivElement>;
  handleProps?: HTMLAttributes<HTMLDivElement>;
}

export function ViewTab({
  id,
  panelId,
  title,
  icon,
  about,
  active,
  tabbable,
  unsaved = false,
  kept = false,
  pinned = false,
  closable = true,
  compact = false,
  dropBefore = false,
  carried = false,
  menu,
  onSelect,
  onClose,
  onDoubleClick,
  handleRef,
  handleProps,
}: ViewTabProps) {
  const { t } = useTranslation();
  const said = about === undefined ? title : `${title} · ${about}`;
  // A narrow tab keeps its title and moves its close mark to the menu.
  const marks = !compact && (closable || unsaved || kept || pinned);
  return (
    // The menu belongs to the whole tab: a right click anywhere on it opens it, and so do the menu
    // key and Shift+F10 while the tab has the keyboard.
    <ContextMenu entries={menu} label={t('workbench.tab.menu', { title })}>
      <div
        role="presentation"
        className={cx(
          'group/tab relative flex h-(--tab-strip-height) min-w-0 max-w-full shrink-0 items-stretch border-r border-border',
          active ? 'bg-bg text-text' : 'text-text-muted hover:bg-bg-muted hover:text-text',
          carried && 'opacity-50',
        )}
      >
        {dropBefore && (
          <span aria-hidden className="absolute inset-y-1 -left-px z-(--layer-sticky) w-0.5 rounded-full bg-accent" />
        )}
        <Tooltip content={compact ? said : about} side="bottom">
          <div
            role="tab"
            id={id}
            aria-selected={active}
            aria-controls={panelId}
            aria-label={unsaved ? t('workbench.tab.unsaved', { title }) : undefined}
            tabIndex={tabbable ? 0 : -1}
            {...handleProps}
            ref={handleRef}
            className={cx(
              'focus-inset relative flex min-w-0 cursor-default items-center gap-2 text-sm select-none',
              compact ? 'px-2' : marks ? 'pr-1 pl-3' : 'px-3',
            )}
            onClick={onSelect}
            onDoubleClick={onDoubleClick}
            onAuxClick={(event) => {
              // The middle button closes a tab, as it does in every browser.
              if (event.button === 1 && closable && !pinned) {
                event.preventDefault();
                onClose();
              }
            }}
            onKeyDown={(event) => {
              handleProps?.onKeyDown?.(event);
              if (event.defaultPrevented) return;
              if (event.key === 'Enter') {
                event.preventDefault();
                onSelect();
              } else if (event.key === 'Delete' && closable && !pinned) {
                event.preventDefault();
                onClose();
              }
            }}
          >
            <span className={cx('flex shrink-0', active && 'text-accent-text')}>{icon}</span>
            <span data-view-tab-title className="min-w-0 truncate">{title}</span>
            {compact && unsaved && (
              <span aria-hidden className="absolute top-2 right-1 size-1 rounded-full bg-text-muted" />
            )}
          </div>
        </Tooltip>
        {/*
          The marks of a tab share one place: the cross takes it when the pointer is on the tab. They
          are for the pointer alone. A strip of tabs holds tabs and nothing else for a screen reader,
          which closes a tab with Delete or from its menu, and is told of what is unsaved by the
          tab's own name.
        */}
        {marks && (
          <span aria-hidden className="relative flex w-6 shrink-0 items-center justify-center">
            {(unsaved || kept || pinned) && (
              <span
                data-pinned={pinned || undefined}
                className={cx('flex items-center justify-center text-text-subtle', closable && !pinned && 'group-hover/tab:invisible')}
              >
                {unsaved ? <span className="size-2 rounded-full bg-text-muted" /> : <Pin size={12} />}
              </span>
            )}
            {closable && !pinned && (
              <button
                type="button"
                tabIndex={-1}
                aria-label={t('workbench.tab.closeNamed', { title })}
                className={cx(
                  'absolute inset-0 m-auto flex size-5 items-center justify-center rounded-sm text-text-muted transition-colors hover:bg-border hover:text-text',
                  (unsaved || kept || !active) && 'invisible group-hover/tab:visible',
                )}
                onClick={onClose}
              >
                <X size={14} aria-hidden />
              </button>
            )}
          </span>
        )}
        {active && <span aria-hidden className="absolute inset-x-0 top-0 h-0.5 bg-accent" />}
        {!compact && <Menu label={t('workbench.tab.menu', { title })} entries={menu} trigger={<button type="button" tabIndex={-1} aria-label={t('workbench.tab.menu', { title })}
          className={cx('flex w-5 shrink-0 items-center justify-center rounded-md text-text-muted hover:bg-bg-muted hover:text-text', !active && 'opacity-0 group-hover/tab:opacity-100 group-focus-within/tab:opacity-100')}><Ellipsis className="size-4" aria-hidden /></button>} />}
      </div>
    </ContextMenu>
  );
}
