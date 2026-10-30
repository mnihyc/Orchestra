/** The page as the browser draws it, scaled to the panel with its proportions kept; and for a person who drives, their pointer, wheel and keys sent to the page in the page's own coordinates.
 *
 * Until Drive is pressed the picture takes nothing: no click, no key, no wheel reaches the page, so that
 * nobody types into a page by passing over it. While driving, the keys go to the page and Orch's own are
 * off inside the picture, all but Ctrl+Alt+Esc, which gives them back. What an input method or a paste
 * gives is sent as text. On a touch screen a tap is a click and a drag scrolls; the keyboard button
 * brings the device's keyboard. Each picture is acknowledged once it is on the screen, so that the next
 * comes; the panel's size is told when it has stood still for a moment, not at every pixel of a drag.
 */
import { Keyboard } from 'lucide-react';
import { type KeyboardEvent as ReactKeyboardEvent, type RefObject, useEffect, useImperativeHandle, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { graphemeCount } from '@/lib/grapheme';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';

import { type BrowserConnection, type BrowserControlFrame, type BrowserPicture, modifiers } from './socket';

/** How long the panel's size must stand still before it is told: a drag of the border sends one size, not a hundred. */
export const SIZE_SETTLES_MS = 300;
/** How far a finger may move and still tap, in CSS pixels. */
const TAP_SLOP_PX = 8;
/** How long a tap may last. */
const TAP_MOST_MS = 400;

const BUTTONS = ['left', 'middle', 'right'] as const;

/** The largest box of the page's proportions that fits the room. */
export function fit(room: { width: number; height: number }, page: { width: number; height: number }): { width: number; height: number } {
  if (room.width <= 0 || room.height <= 0 || page.width <= 0 || page.height <= 0) return { width: 0, height: 0 };
  const scale = Math.min(room.width / page.width, room.height / page.height);
  return { width: Math.floor(page.width * scale), height: Math.floor(page.height * scale) };
}

/** A point of the picture on the screen, in the page's own coordinates, whatever size the picture has. */
export function toPage(point: { x: number; y: number }, box: { left: number; top: number; width: number; height: number }, page: { width: number; height: number }): { x: number; y: number } {
  const x = box.width === 0 ? 0 : ((point.x - box.left) / box.width) * page.width;
  const y = box.height === 0 ? 0 : ((point.y - box.top) / box.height) * page.height;
  return { x: Math.round(Math.min(Math.max(x, 0), page.width - 1)), y: Math.round(Math.min(Math.max(y, 0), page.height - 1)) };
}

export interface PictureProps {
  connection: BrowserConnection;
  /** Where the connection hands each picture; the picture draws it. */
  sink: RefObject<((picture: BrowserPicture) => void) | null>;
  tab: string | null;
  viewport: { width: number; height: number } | null;
  driving: boolean;
  /** Ctrl+Alt+Esc: the keys go back to Orch. */
  onGiveBack: () => void;
  /** The panel's size, as last told, for "Fit window to this panel". */
  onPanel?: (size: { width: number; height: number }) => void;
  label: string;
}

export function Picture({ connection, sink, tab, viewport, driving, onGiveBack, onPanel, label }: PictureProps) {
  const { t } = useTranslation();
  const room = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const keys = useRef<HTMLTextAreaElement>(null);
  const draw = useRef<((picture: BrowserPicture) => void) | null>(null);
  // React installs and clears the parent's handle; this effect owns the drawing callback.
  useImperativeHandle(sink, () => (picture: BrowserPicture) => draw.current?.(picture), []);
  const [space, setSpace] = useState({ width: 0, height: 0 });
  const [drawnOnce, setDrawnOnce] = useState(false);
  const page = viewport ?? { width: 1280, height: 800 };
  const shown = fit(space, page);

  // The room the picture has, followed as the panel changes.
  useEffect(() => {
    const element = room.current;
    if (element === null) return;
    const observer = new ResizeObserver(([entry]) => {
      if (entry !== undefined) setSpace({ width: entry.contentRect.width, height: entry.contentRect.height });
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  // The size is told once it has stood still: what a drag of the border passes through is not.
  const shownWidth = shown.width;
  const shownHeight = shown.height;
  useEffect(() => {
    if (tab === null || shownWidth === 0) return;
    const timer = setTimeout(() => {
      connection.watch(tab, shownWidth, shownHeight);
      onPanel?.({ width: shownWidth, height: shownHeight });
    }, SIZE_SETTLES_MS);
    return () => clearTimeout(timer);
  }, [connection, tab, shownWidth, shownHeight, onPanel]);

  // Each picture is drawn as it comes, and said to be drawn once it is on the screen.
  useEffect(() => {
    const lifetime = new AbortController();
    draw.current = (picture) => {
      const target = canvas.current;
      const blob = new Blob([picture.jpeg], { type: 'image/jpeg' });
      void createImageBitmap(blob)
        .then((bitmap) => {
          if (lifetime.signal.aborted || target === null) {
            bitmap.close();
            return;
          }
          if (target.width !== bitmap.width || target.height !== bitmap.height) {
            target.width = bitmap.width;
            target.height = bitmap.height;
          }
          target.getContext('2d')?.drawImage(bitmap, 0, 0);
          bitmap.close();
          setDrawnOnce(true);
        })
        .catch(() => undefined)
        .finally(() => {
          if (!lifetime.signal.aborted) requestAnimationFrame(() => {
            if (!lifetime.signal.aborted) connection.drawn(picture.n);
          });
        });
    };
    return () => {
      lifetime.abort();
      draw.current = null;
    };
  }, [connection]);

  // Pointer and wheel, as the page has them; nothing at all while watching.
  const pageWidth = page.width;
  const pageHeight = page.height;
  useEffect(() => {
    const target = canvas.current;
    if (target === null || !driving || tab === null) return;
    const page = { width: pageWidth, height: pageHeight };
    const send = (frame: BrowserControlFrame) => connection.send(frame);
    const at = (event: { clientX: number; clientY: number }) => toPage({ x: event.clientX, y: event.clientY }, target.getBoundingClientRect(), page);
    let pendingMove: PointerEvent | null = null;
    let frame = 0;
    let touch: { x: number; y: number; at: number; moved: boolean; last: { x: number; y: number } } | null = null;
    const flushMove = () => {
      frame = 0;
      if (pendingMove === null) return;
      const event = pendingMove;
      pendingMove = null;
      send({ op: 'pointer', tab, kind: 'move', ...at(event), buttons: event.buttons, mod: modifiers(event) });
    };
    const down = (event: PointerEvent) => {
      keys.current?.focus({ preventScroll: true });
      if (event.pointerType === 'touch') {
        touch = { x: event.clientX, y: event.clientY, at: performance.now(), moved: false, last: { x: event.clientX, y: event.clientY } };
        return;
      }
      event.preventDefault();
      target.setPointerCapture(event.pointerId);
      send({ op: 'pointer', tab, kind: 'down', ...at(event), button: BUTTONS[event.button] ?? 'left', buttons: event.buttons, clicks: event.detail, mod: modifiers(event) });
    };
    const move = (event: PointerEvent) => {
      if (event.pointerType === 'touch') {
        if (touch === null) return;
        if (Math.hypot(event.clientX - touch.x, event.clientY - touch.y) > TAP_SLOP_PX) touch.moved = true;
        if (touch.moved) {
          // A drag scrolls: the finger moves the page as it would on the phone itself.
          const scale = page.width / Math.max(1, target.getBoundingClientRect().width);
          send({ op: 'pointer', tab, kind: 'wheel', ...at(event), dx: Math.round((touch.last.x - event.clientX) * scale), dy: Math.round((touch.last.y - event.clientY) * scale) });
          touch.last = { x: event.clientX, y: event.clientY };
        }
        return;
      }
      pendingMove = event;
      if (frame === 0) frame = requestAnimationFrame(flushMove);
    };
    const up = (event: PointerEvent) => {
      if (event.pointerType === 'touch') {
        const tapped = touch !== null && !touch.moved && performance.now() - touch.at <= TAP_MOST_MS;
        touch = null;
        if (tapped) {
          const point = at(event);
          send({ op: 'pointer', tab, kind: 'move', ...point });
          send({ op: 'pointer', tab, kind: 'down', ...point, button: 'left', buttons: 1, clicks: 1 });
          send({ op: 'pointer', tab, kind: 'up', ...point, button: 'left', buttons: 0, clicks: 1 });
        }
        return;
      }
      flushMove();
      send({ op: 'pointer', tab, kind: 'up', ...at(event), button: BUTTONS[event.button] ?? 'left', buttons: event.buttons, clicks: event.detail, mod: modifiers(event) });
    };
    const wheel = (event: WheelEvent) => {
      event.preventDefault();
      send({ op: 'pointer', tab, kind: 'wheel', ...at(event), dx: Math.round(event.deltaX), dy: Math.round(event.deltaY), mod: modifiers(event) });
    };
    const menu = (event: Event) => event.preventDefault();
    target.addEventListener('pointerdown', down);
    target.addEventListener('pointermove', move);
    target.addEventListener('pointerup', up);
    target.addEventListener('wheel', wheel, { passive: false });
    target.addEventListener('contextmenu', menu);
    return () => {
      cancelAnimationFrame(frame);
      target.removeEventListener('pointerdown', down);
      target.removeEventListener('pointermove', move);
      target.removeEventListener('pointerup', up);
      target.removeEventListener('wheel', wheel);
      target.removeEventListener('contextmenu', menu);
    };
  }, [connection, driving, tab, pageWidth, pageHeight]);

  // Driving takes the keys at once; watching gives them back.
  useEffect(() => {
    if (driving) keys.current?.focus({ preventScroll: true });
  }, [driving]);

  const key = (kind: 'down' | 'up') => (event: ReactKeyboardEvent<HTMLTextAreaElement>) => {
    if (!driving || tab === null) return;
    if (kind === 'down' && event.ctrlKey && event.altKey && event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      onGiveBack();
      return;
    }
    // An input method's keys are its own until it hands over its text.
    if (event.nativeEvent.isComposing || event.key === 'Process' || event.key === 'Dead') return;
    // A paste is sent as its text, by the paste itself.
    if (kind === 'down' && (event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'v') return;
    event.preventDefault();
    event.stopPropagation();
    const text = kind === 'down' && graphemeCount(event.key) === 1 && !event.ctrlKey && !event.metaKey ? event.key : undefined;
    connection.send({ op: 'key', tab, kind, key: event.key, code: event.code, ...(text === undefined ? {} : { text }), mod: modifiers(event), repeat: event.repeat, location: event.location });
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-1">
      <div ref={room} className="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden bg-bg-muted focus-within:outline-2 focus-within:outline-focus" data-browser-picture={driving ? 'driving' : 'watching'}>
        <canvas
          ref={canvas}
          role="img"
          aria-label={label}
          width={page.width}
          height={page.height}
          style={{ width: shown.width, height: shown.height, touchAction: driving ? 'none' : 'auto' }}
          className={cx('block bg-surface', driving ? 'cursor-default' : 'cursor-not-allowed')}
        />
        {!drawnOnce && <span className="absolute text-sm text-text-muted">{t('browser.picture.waiting')}</span>}
        <textarea
          ref={keys}
          aria-label={t('browser.picture.keys')}
          tabIndex={driving ? 0 : -1}
          readOnly={!driving}
          className="pointer-events-none absolute inset-0 resize-none opacity-0"
          onKeyDown={key('down')}
          onKeyUp={key('up')}
          onCompositionEnd={(event) => {
            if (!driving || tab === null || event.data === '') return;
            connection.send({ op: 'text', tab, text: event.data });
            event.currentTarget.value = '';
          }}
          onPaste={(event) => {
            if (!driving || tab === null) return;
            event.preventDefault();
            const text = event.clipboardData.getData('text/plain');
            if (text !== '') connection.send({ op: 'text', tab, text });
          }}
          onInput={(event) => {
            // What the keys did not say themselves (a phone's keyboard) arrives here as text.
            const value = event.currentTarget.value;
            if (driving && tab !== null && value !== '' && !event.nativeEvent.isComposing) {
              connection.send({ op: 'text', tab, text: value });
              event.currentTarget.value = '';
            }
          }}
        />
      </div>
      {driving && (
        <div className="flex justify-end sm:hidden">
          <Button size="sm" variant="ghost" icon={<Keyboard aria-hidden />} onClick={() => keys.current?.focus()}>
            {t('browser.picture.keyboard')}
          </Button>
        </div>
      )}
    </div>
  );
}
