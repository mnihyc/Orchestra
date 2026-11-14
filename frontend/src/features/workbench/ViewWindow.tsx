/** A view's existing portal home moves between its dock and a modeless window without remounting. */
import { Expand, Move, PanelRightClose, Shrink, SquareArrowOutUpRight, X } from 'lucide-react';
import { type KeyboardEvent, type PointerEvent, type ReactNode, useCallback, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { useViewChoice } from '@/lib/viewChoices';
import { isComposing } from '@/lib/shortcuts';
import { FLOATING_EDGE } from '@/ui/floating';
import { IconButton } from '@/ui/IconButton';
import { Button } from '@/ui/Button';
import { StateLine } from '@/ui/StateLine';
import { useFrameReady } from '@/shell/FrameReadiness';
import { tabHidden, type Tab } from './layout';
import { openingForView, showViewOpening } from './viewOpening';
import { viewKey } from './viewIdentity';
import { useWorkbench } from './store';

/** A floating slot remains visible when its former bar closes, including on a restored workspace; it is gone when its own section is closed. */
export function ViewWindowLease({ tab }: { tab: Tab }) {
  const ready = useFrameReady();
  const [floating] = useViewChoice<boolean>('view-window', 'floating', false, tab.view);
  const hidden = useWorkbench((state) => tabHidden(state.layout, tab.id));
  const key = viewKey(tab);
  useLayoutEffect(() => floating && ready && !hidden ? showViewOpening(key, openingForView(tab.id)) : undefined, [floating, ready, hidden, key, tab.id]);
  return null;
}

export interface WindowGeometry { x: number; y: number; width: number; height: number }
// The workbench's 320px bar minimum, a 420px reading width and 640px of conversation to start.
const DEFAULT_WINDOW: WindowGeometry = { x: 64, y: 64, width: 420, height: 640 };
export function fitViewWindow(value: WindowGeometry, width: number, height: number, minimumHeight = 240): WindowGeometry {
  const availableWidth = Math.max(0, width - 2 * FLOATING_EDGE), availableHeight = Math.max(0, height - 2 * FLOATING_EDGE);
  const w = Math.min(availableWidth, Math.max(320, value.width));
  const h = Math.min(availableHeight, Math.max(minimumHeight, value.height));
  return { width: w, height: h, x: Math.max(FLOATING_EDGE, Math.min(value.x, width - FLOATING_EDGE - w)), y: Math.max(FLOATING_EDGE, Math.min(value.y, height - FLOATING_EDGE - h)) };
}
function readGeometry(text: string): WindowGeometry {
  try {
    const value: unknown = JSON.parse(text);
    if (typeof value === 'object' && value !== null && 'x' in value && 'y' in value && 'width' in value && 'height' in value
      && [value.x, value.y, value.width, value.height].every((part) => typeof part === 'number' && Number.isFinite(part))) return value as WindowGeometry;
  } catch { /* A malformed saved choice uses the shipped geometry. */ }
  return DEFAULT_WINDOW;
}

export function ViewWindow({ name, tabId, title, actions, status, children, onClose }: {
  name: string; tabId?: string; title: string; actions?: ReactNode; status?: ReactNode; children: ReactNode; onClose: () => void;
}) {
  const { t } = useTranslation();
  const [floating, float] = useViewChoice<boolean>('view-window', 'floating', false, name);
  const [maximized, maximize] = useViewChoice<boolean>('view-window', 'maximized', false, name);
  const [saved, save] = useViewChoice('view-window', 'geometry', JSON.stringify(DEFAULT_WINDOW), name);
  const [geometry, setGeometry] = useState(() => fitViewWindow(readGeometry(saved), innerWidth, innerHeight));
  const actual = useRef(geometry);
  const minimumHeight = useRef(240);
  const header = useRef<HTMLDivElement>(null), body = useRef<HTMLDivElement>(null);
  const dock = useRef<HTMLDivElement>(null);
  const [home] = useState(() => document.createElement('div'));
  // Closing the window closes the view's section: a floating window of a closed section is not shown, and comes back with the section.
  const closed = useWorkbench((state) => tabId !== undefined && tabHidden(state.layout, tabId));
  const drag = useRef<{ mode: 'move' | 'resize'; x: number; y: number; start: WindowGeometry } | null>(null);
  const update = useCallback((next: WindowGeometry) => {
    actual.current = fitViewWindow(next, innerWidth, innerHeight, minimumHeight.current);
    setGeometry((before) => Object.keys(before).every((key) => before[key as keyof WindowGeometry] === actual.current[key as keyof WindowGeometry]) ? before : actual.current);
  }, []);
  useLayoutEffect(() => { update(readGeometry(saved)); }, [saved, update]);
  useLayoutEffect(() => {
    const resize = () => update(actual.current);
    window.addEventListener('resize', resize);
    return () => window.removeEventListener('resize', resize);
  }, [update]);
  useLayoutEffect(() => {
    const parent = floating ? closed ? null : document.body : dock.current;
    parent?.appendChild(home);
    return () => home.remove();
  }, [floating, closed, home]);
  // Escape in a floating window closes it, unless something inside it (a menu, a list, a field that was composing) took the key.
  useLayoutEffect(() => {
    if (!floating) return;
    const escape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented || isComposing(event)) return;
      event.preventDefault(); event.stopPropagation(); onClose();
    };
    home.addEventListener('keydown', escape);
    return () => home.removeEventListener('keydown', escape);
  }, [floating, home, onClose]);
  useLayoutEffect(() => {
    if (tabId === undefined) return;
    const focus = () => useWorkbench.getState().focus(tabId);
    home.addEventListener('focusin', focus); home.addEventListener('pointerdown', focus);
    return () => { home.removeEventListener('focusin', focus); home.removeEventListener('pointerdown', focus); };
  }, [home, tabId]);
  useLayoutEffect(() => {
    home.setAttribute('class', floating ? 'fixed z-(--layer-overlay) flex min-h-0 flex-col overflow-hidden rounded-lg border border-border-strong bg-surface text-text shadow-lg' : 'flex h-full min-h-0 flex-col');
    home.setAttribute('data-view-window', name);
    home.setAttribute('data-floating', String(floating));
    home.setAttribute('role', floating ? 'dialog' : 'region');
    home.setAttribute('aria-label', title);
    const box = maximized ? { left: `${FLOATING_EDGE}px`, top: `${FLOATING_EDGE}px`, width: `calc(100dvw - ${2 * FLOATING_EDGE}px)`, height: `calc(100dvh - ${2 * FLOATING_EDGE}px)` }
      : { left: `${geometry.x}px`, top: `${geometry.y}px`, width: `${geometry.width}px`, height: `${geometry.height}px` };
    Object.assign(home.style, floating ? box : { left: '', top: '', width: '', height: '' });
  }, [floating, maximized, geometry, home, name, title]);
  useLayoutEffect(() => {
    if (header.current === null || body.current === null || typeof ResizeObserver === 'undefined') return;
    const chrome = [header.current, ...Array.from(body.current.children).filter((node): node is HTMLElement => node instanceof HTMLElement && getComputedStyle(node).flexShrink === '0')];
    const measure = () => {
      // Keep every fixed control and one comfortable conversation row visible when resizing.
      minimumHeight.current = Math.max(240, chrome.reduce((height, node) => height + node.getBoundingClientRect().height, 0) + 32);
      update(actual.current);
    };
    const observer = new ResizeObserver(measure); for (const node of chrome) observer.observe(node); measure();
    return () => observer.disconnect();
  }, [home, floating, update]);
  const begin = (event: PointerEvent<HTMLElement>, mode: 'move' | 'resize') => {
    if (!floating || maximized || event.button !== 0) return;
    event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId);
    drag.current = { mode, x: event.clientX, y: event.clientY, start: actual.current };
  };
  const move = (event: PointerEvent<HTMLElement>) => {
    const held = drag.current;
    if (held === null) return;
    const dx = event.clientX - held.x, dy = event.clientY - held.y;
    update(held.mode === 'move' ? { ...held.start, x: held.start.x + dx, y: held.start.y + dy } : { ...held.start, width: held.start.width + dx, height: held.start.height + dy });
  };
  const finish = () => { if (drag.current !== null) { drag.current = null; save(JSON.stringify(actual.current)); } };
  const keyMove = (event: KeyboardEvent<HTMLElement>, resize: boolean) => {
    if (!floating || maximized || isComposing(event.nativeEvent) || !['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight'].includes(event.key)) return;
    event.preventDefault();
    // One spacing step per key; Shift offers a larger, still reversible move.
    const step = event.shiftKey ? 32 : 8;
    const dx = event.key === 'ArrowLeft' ? -step : event.key === 'ArrowRight' ? step : 0;
    const dy = event.key === 'ArrowUp' ? -step : event.key === 'ArrowDown' ? step : 0;
    update(resize ? { ...actual.current, width: actual.current.width + dx, height: actual.current.height + dy } : { ...actual.current, x: actual.current.x + dx, y: actual.current.y + dy });
    save(JSON.stringify(actual.current));
  };
  return <><div ref={dock} className="h-full min-h-0">{floating && <StateLine compact title={t('viewWindow.floating')} action={<Button size="sm" onClick={() => float(false)}>{t('viewWindow.dock')}</Button>} />}</div>
    {createPortal(<>
      <div ref={header} className="flex min-w-0 shrink-0 flex-wrap items-center gap-1 border-b border-border bg-bg-subtle px-2 py-1">
        {floating && !maximized ? <button type="button" className="focus-inset flex min-w-0 flex-1 touch-none items-center gap-1 text-left text-sm font-medium" aria-label={t('viewWindow.move', { name: title })}
          onPointerDown={(event) => begin(event, 'move')} onPointerMove={move} onPointerUp={finish} onPointerCancel={finish} onKeyDown={(event) => keyMove(event, false)}>
          <Move className="size-4 shrink-0 text-text-muted" aria-hidden /><span className="truncate">{title}</span>{status}
        </button> : <div className="flex min-w-0 flex-1 items-center gap-1 text-sm font-medium"><span className="truncate">{title}</span>{status}</div>}
        <div className="ml-auto flex max-w-full flex-wrap items-center gap-1">{actions}
          <IconButton size="sm" label={t(floating ? 'viewWindow.dock' : 'viewWindow.popOut')} icon={floating ? <PanelRightClose aria-hidden /> : <SquareArrowOutUpRight aria-hidden />} onClick={() => float(!floating)} />
          {floating && <><IconButton size="sm" label={t(maximized ? 'viewWindow.restore' : 'viewWindow.maximize')} icon={maximized ? <Shrink aria-hidden /> : <Expand aria-hidden />} onClick={() => maximize(!maximized)} />
            <IconButton size="sm" label={t('common.close')} icon={<X aria-hidden />} onClick={onClose} /></>}
        </div>
      </div>
      <div ref={body} className="flex min-h-0 flex-1 flex-col">{children}</div>
      {floating && !maximized && <button type="button" aria-label={t('viewWindow.resize', { name: title })} className="focus-inset absolute right-0 bottom-0 size-4 cursor-se-resize touch-none border-r-2 border-b-2 border-text-muted"
        onPointerDown={(event) => begin(event, 'resize')} onPointerMove={move} onPointerUp={finish} onPointerCancel={finish} onKeyDown={(event) => keyMove(event, true)} />}
    </>, home)}
  </>;
}
