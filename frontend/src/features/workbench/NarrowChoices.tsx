/** Phone containers share one icon row; medium-width containers keep their short visible names. */
import { type FocusEventHandler, type KeyboardEvent, type ReactNode, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { Tooltip } from '@/ui/Tooltip';
import { OVERFLOW_REST_MS } from '@/ui/overflowTooltips';

export interface NarrowChoice {
  id: string;
  name: string;
  description: string;
  icon: ReactNode;
  active: boolean;
  onSelect: () => void;
}

/** Two spacing steps allow finger jitter while cancelling a hold that becomes a scroll. */
const HOLD_SLOP = 8;

function PhoneChoice({ choice, controls, tabIndex, onFocus }: { choice: NarrowChoice; controls: string | undefined; tabIndex: number; onFocus: FocusEventHandler<HTMLButtonElement> }) {
  const [open, setOpen] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const finger = useRef<{ x: number; y: number } | null>(null);
  const held = useRef(false);
  const stop = () => { if (timer.current !== null) clearTimeout(timer.current); timer.current = null; finger.current = null; };
  useEffect(() => () => { if (timer.current !== null) clearTimeout(timer.current); }, []);
  return <Tooltip content={choice.name} side="bottom" open={open} onOpenChange={setOpen}>
    <button type="button" id={`container-choice-${choice.id}`} data-container-choice={choice.id}
      aria-label={choice.name} aria-description={choice.description} aria-pressed={choice.active} aria-controls={choice.active ? controls : undefined}
      tabIndex={tabIndex} onFocus={onFocus}
      className="choice-option focus-inset relative inline-flex h-11 w-full min-w-0 max-w-11 touch-manipulation items-center justify-center justify-self-center rounded-md text-text-muted hover:bg-bg-muted hover:text-text aria-pressed:after:absolute aria-pressed:after:inset-x-2 aria-pressed:after:bottom-1 aria-pressed:after:h-0.5 aria-pressed:after:bg-accent"
      onPointerDown={(event) => {
        if (event.pointerType !== 'touch') return;
        stop(); held.current = false; finger.current = { x: event.clientX, y: event.clientY };
        // Use the ordinary name-rest delay; a quick tap still opens the view immediately.
        timer.current = setTimeout(() => { timer.current = null; held.current = true; setOpen(true); }, OVERFLOW_REST_MS);
      }}
      onPointerMove={(event) => {
        const start = finger.current;
        if (start !== null && Math.hypot(event.clientX - start.x, event.clientY - start.y) > HOLD_SLOP) { stop(); setOpen(false); }
      }}
      onPointerUp={stop}
      onPointerCancel={() => { stop(); held.current = false; setOpen(false); }}
      onPointerLeave={() => { stop(); setOpen(false); }}
      onContextMenu={(event) => { if (finger.current !== null || held.current) event.preventDefault(); }}
      onBlur={() => { stop(); held.current = false; }}
      onClick={(event) => {
        const readName = held.current; held.current = false;
        if (readName && event.detail > 0) { event.preventDefault(); return; }
        choice.onSelect();
      }}>
      <span aria-hidden className="flex size-5 shrink-0 items-center justify-center">{choice.icon}</span>
      <span data-container-name className="sr-only">{choice.name}</span>
    </button>
  </Tooltip>;
}

export function NarrowChoices({ choices, presentation = 'named' }: { choices: readonly NarrowChoice[]; presentation?: 'named' | 'icons' }) {
  const { t } = useTranslation();
  const root = useRef<HTMLDivElement>(null);
  if (choices.length === 0) return null;
  const active = choices.findIndex((choice) => choice.active);
  const buttons = () => Array.from(root.current?.querySelectorAll<HTMLButtonElement>('button[data-container-choice]') ?? []);
  const rove = (target: HTMLButtonElement) => { for (const button of buttons()) button.tabIndex = button === target ? 0 : -1; };
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey) return;
    const items = buttons();
    const at = items.findIndex((item) => item === event.target);
    if (at < 0) return;
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1
      : event.key === 'ArrowRight' || event.key === 'ArrowDown' ? (at + 1) % items.length
        : event.key === 'ArrowLeft' || event.key === 'ArrowUp' ? (at - 1 + items.length) % items.length : null;
    const target = next === null ? undefined : items[next];
    if (target !== undefined) { event.preventDefault(); rove(target); target.focus(); }
  };
  return <div ref={root} role="toolbar" aria-label={t('workbench.containers.label')} data-area-stop tabIndex={-1} data-narrow-containers data-container-presentation={presentation} data-navigation-strip="containers"
    className={cx('min-w-0 border-b border-border bg-bg-subtle px-1', presentation === 'icons' ? 'grid grid-flow-col auto-cols-fr items-center' : 'flex flex-wrap items-start gap-1 py-1')} onKeyDown={onKeyDown}>
    {choices.map((choice, index) => {
      const split = choice.id.indexOf(':');
      const bar = choice.id.slice(0, split);
      const controls = bar === 'left' || bar === 'right' ? `container-panel-${bar}-${choice.id.slice(split + 1)}` : undefined;
      if (presentation === 'icons') return <PhoneChoice key={choice.id} choice={choice} controls={controls}
        tabIndex={index === (active < 0 ? 0 : active) ? 0 : -1} onFocus={(event) => rove(event.currentTarget)} />;
      return <Button key={choice.id} id={`container-choice-${choice.id}`} size="sm" variant="ghost" icon={<span className="flex h-5 shrink-0 items-center">{choice.icon}</span>}
      data-container-choice={choice.id} aria-label={choice.name} aria-description={choice.description} aria-pressed={choice.active} aria-controls={choice.active ? controls : undefined}
      tabIndex={index === (active < 0 ? 0 : active) ? 0 : -1} onFocus={(event) => rove(event.currentTarget)} onClick={choice.onSelect}
      className="choice-option h-auto min-h-(--control-sm) max-w-full items-start py-1 aria-pressed:underline aria-pressed:decoration-accent aria-pressed:underline-offset-4">
      <span data-container-name className="min-w-0 text-left leading-5 whitespace-normal wrap-anywhere">{choice.name}</span>
    </Button>;
    })}
  </div>;
}
