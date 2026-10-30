/** Below the large width the right bar over the main area; below the medium width one area at a time, as sheets.
 *
 * The workbench has three shapes. With room for everything, its areas stand beside one another.
 * With less, the right bar no longer fits beside the main area and lies over it when it is opened.
 * On a small screen there are no areas at all: the screen of the route, and one other view at a
 * time as a sheet over it. Which shape fits is asked of the page's own widths, which are tokens.
 */
import { createContext, useContext, type ReactNode, useSyncExternalStore } from 'react';
import { useTranslation } from 'react-i18next';

import { Drawer } from '@/ui/Drawer';
import { activeContainer, type SideBar, type SideBarId, type Tab } from '@/features/workbench/layout';
import { containerOf } from '@/features/workbench/containers';
import { SideBarView } from '@/features/workbench/Sections';
import { viewOf } from '@/features/workbench/views';
import { X } from 'lucide-react';
import { IconButton } from '@/ui/IconButton';
import { useChromeBottom } from '@/features/workbench/chrome';

export type Shape =
  /** Every area beside the others. */
  | 'wide'
  /** The right bar over the main area. */
  | 'medium'
  /** No areas: one view at a time. */
  | 'small';

/** The widths, as written in the tokens of the page; the fallbacks are what they come to at the usual text size. */
function query(token: '--screen-md' | '--screen-lg' | '--breakpoint-sm', fallback: string): MediaQueryList | null {
  if (typeof matchMedia !== 'function') return null;
  const width = getComputedStyle(document.documentElement).getPropertyValue(token).trim() || fallback;
  return matchMedia(`(min-width: ${width})`);
}

let medium: MediaQueryList | null | undefined;
let large: MediaQueryList | null | undefined;

function shape(): Shape {
  medium ??= query('--screen-md', '900px');
  large ??= query('--screen-lg', '1200px');
  // Where the page cannot be asked (a test without a window to measure), everything has room.
  if (medium === null || large === null) return 'wide';
  return large.matches ? 'wide' : medium.matches ? 'medium' : 'small';
}

function subscribe(listener: () => void): () => void {
  shape();
  medium?.addEventListener('change', listener);
  large?.addEventListener('change', listener);
  return () => {
    medium?.removeEventListener('change', listener);
    large?.removeEventListener('change', listener);
  };
}

export const ViewShapeContext = createContext<Shape | null>(null);
export const ViewCompactContext = createContext<boolean | null>(null);
let controls: MediaQueryList | null | undefined;
function compactControls(): boolean { controls ??= query('--breakpoint-sm', '640px'); return controls !== null && !controls.matches; }
function watchControls(listener: () => void): () => void {
  compactControls(); controls?.addEventListener('change', listener);
  return () => controls?.removeEventListener('change', listener);
}
/** Reuse the existing sm control presentation at the cell's width, including a wide browser's narrow cells. */
export function useCompactControls(): boolean {
  const cell = useContext(ViewCompactContext);
  const windowCompact = useSyncExternalStore(watchControls, compactControls, () => false);
  return cell ?? windowCompact;
}


/** Content measures its cell; the shell outside a view still measures the browser. */
export function useShape(): Shape {
  const cell = useContext(ViewShapeContext);
  const windowShape = useViewportShape();
  return cell ?? windowShape;
}
/** Device-wide interaction rules, such as phone file editing, do not change when a desktop cell is resized. */
export function useViewportShape(): Shape { return useSyncExternalStore<Shape>(subscribe, shape, () => 'wide'); }

export interface OverMainProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  children: ReactNode;
}

/** The right bar where it has no room beside the main area: a sheet over it, from the right. */
export function RightBarOverMain({ open, onOpenChange, children }: OverMainProps) {
  const { t } = useTranslation();
  const top = useChromeBottom();
  return (
    <Drawer prompt={false} modal={false} persistent side="right" open={open} onOpenChange={onOpenChange} title={t('workbench.areas.right')} hideTitle scroll={false}
      style={top === undefined ? undefined : { top, height: 'auto' }}>
      {children}
    </Drawer>
  );
}

/** A phone sheet draws the same named container and section controls as its desktop bar. */
export function ContainerSheet({ bar, layout, open, onOpenChange, onSelect, renderView, side = 'bottom', restoreFocus = true, top, externalSwitcher = false }: {
  bar: SideBarId; layout: SideBar; open: boolean; onOpenChange: (open: boolean) => void;
  onSelect: (name: string) => void; renderView: (tab: Tab) => ReactNode; side?: 'left' | 'bottom'; restoreFocus?: boolean; top?: number; externalSwitcher?: boolean;
}) {
  const { t } = useTranslation();
  const container = activeContainer(layout);
  const definition = container === null ? undefined : containerOf(container.name);
  const view = viewOf(container?.sections[0]?.tab.view ?? '');
  const title = definition === undefined ? t(view?.title ?? 'views.navigation.title') : t(definition.title);
  return <Drawer prompt={false} side={side} open={open} onOpenChange={onOpenChange} title={title} hideTitle modal={false} persistent restoreFocus={restoreFocus} scroll={false}
    style={top === undefined ? undefined : { top, height: 'auto', maxHeight: 'none' }}
    className={side === 'bottom' ? 'h-[calc(100dvh-var(--control-md))] max-h-none' : undefined}>
    <SideBarView bar={bar} layout={layout} renderView={renderView} addView={null} onSelectContainer={onSelect} externalSwitcher={externalSwitcher}
      actions={<IconButton size="sm" label={t('common.close')} icon={<X aria-hidden />} onClick={() => onOpenChange(false)} />} />
  </Drawer>;
}
