/** The one list that the composer's choices share (model, effort, speed, access program, provider, agent): single-line rows (the name, then its facts in muted words on the same line), the current row checked, groups under small headers that are shown only when they hold rows, a search field once more than eight rows are listed, and its own scroll.
 *
 * A control of the box shows its current value alone. Its list opens below it where the screen has room for it (a
 * dialog) and above it where it does not (the box at the foot of the screen): nothing in it is chosen by moving through
 * it, a row is chosen by pressing it, and in each group the rows that can be chosen come first; one that cannot follows
 * them, greyed, with its reason under its name. Facts about the current choice wait, closed, under one line at the foot.
 * The same list stands in the dialogs that choose a provider, an agent, a model or an effort, so a person meets one way
 * of choosing.
 */
import { Check, ChevronDown, ChevronRight } from 'lucide-react';
import { Fragment, type KeyboardEvent, type ReactNode, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useCompactControls } from '@/features/workbench/Narrow';
import { isComposing } from '@/lib/shortcuts';
import { Button } from '@/ui/Button';
import { Choice, factLine, hasChoiceDescription } from '@/ui/Choice';
import { cx } from '@/ui/cx';
import { Input } from '@/ui/Input';
import { Popover } from '@/ui/Popover';

/** More rows than this get a search field. */
export const SEARCH_FROM = 8;

export interface PickRow {
  id: string;
  /** What the row is called; the words the search reads first. */
  name: string;
  /** Muted words after the name, on the same line: where two rows share a name, the place of each. */
  place?: string | undefined;
  /** The row's fact, after its place on the same line; under the name, as the reason, where the row cannot be chosen. */
  detail?: ReactNode;
  /** A recovery act beside a disabled choice. */
  action?: ReactNode;
  /** More words the search reads. */
  keywords?: string | undefined;
  checked?: boolean;
  disabled?: boolean;
  /** A small mark after the name, for a row that is not ready. */
  mark?: ReactNode;
}

export interface PickGroup {
  id: string;
  /** The small header over the rows; none for a group that stands without one. */
  label: string | null;
  /** The header is shown even where this is the only group listed: for a group whose name is what says why its rows are as they are. */
  always?: boolean;
  rows: readonly PickRow[];
}

const fold = (text: string): string => text.normalize('NFKC').toLocaleLowerCase();

/** The menu keeps one tab stop; moving focus never chooses anything. */
function moveFocus(event: KeyboardEvent<HTMLUListElement>): void {
  if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey || isComposing(event.nativeEvent)
    || !['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
  const rows = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]:not(:disabled)'));
  if (rows.length === 0) return;
  const target = event.target instanceof Node ? event.target : null;
  const at = rows.findIndex((row) => row.contains(target));
  const next = event.key === 'Home' ? 0 : event.key === 'End' ? rows.length - 1
    : at < 0 ? event.key === 'ArrowUp' ? rows.length - 1 : 0 : (at + (event.key === 'ArrowDown' ? 1 : -1) + rows.length) % rows.length;
  event.preventDefault(); event.stopPropagation(); rows[next]?.focus();
}

function Row({ row, tabStop, onFocused, onChoose }: { row: PickRow; tabStop: boolean; onFocused: () => void; onChoose: () => void }) {
  const { t } = useTranslation();
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const disabled = row.disabled === true;
  const name = <span className="flex min-w-0 items-center gap-2">
    <span className="min-w-0 truncate">{row.name}</span>
    {row.mark}
  </span>;
  // One line: the name, then its place and its fact; a reason why a row cannot be chosen is read whole, under the name.
  const inline = !disabled || !hasChoiceDescription(row.detail);
  return <Choice presentation="picker" compact inline={inline} reason={!inline} name={name} description={factLine([row.place, row.detail], t('resourceFacts.separator'))} className={cx('w-full', disabled ? 'text-text-disabled' : 'text-text')}>
    {(content, control) => <button {...control} type="button" role="menuitemradio" aria-checked={row.checked === true} disabled={disabled}
      tabIndex={!disabled && tabStop ? 0 : -1} data-highlighted={!disabled && (hovered || focused) ? '' : undefined}
      onFocus={() => { setFocused(true); onFocused(); }} onBlur={() => setFocused(false)}
      onPointerEnter={() => setHovered(true)} onPointerLeave={() => setHovered(false)} onClick={onChoose}
      className="choice-option flex min-h-8 min-w-0 w-full items-center gap-2 rounded-sm px-2 py-1 text-left text-sm disabled:cursor-default">
      <span aria-hidden className="flex size-4 shrink-0 items-center justify-center text-accent-text">{row.checked === true && <Check aria-hidden />}</span>
      {content}
    </button>}
  </Choice>;
}

export interface PickListProps {
  /** What the list is, for a screen reader. */
  label: string;
  /** What the search field is called; the field stands only where more than eight rows are listed. */
  searchLabel?: string;
  groups: readonly PickGroup[];
  onChoose: (row: PickRow) => void;
  /** What stands above the rows: why a choice cannot be made now. */
  above?: ReactNode;
  /** What stands under the rows. */
  below?: ReactNode;
  /** Facts about the current choice: closed under one line at the foot until asked for, so the rows keep the room. */
  facts?: ReactNode;
  /** Said where the list has no row at all. */
  empty?: ReactNode;
  className?: string;
}

/** The rows of a choice, grouped, searchable and with their own scroll. */
export function PickList({ label, searchLabel, groups, onChoose, above, below, facts, empty, className }: PickListProps) {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const [focusKey, setFocusKey] = useState<string | null>(null);
  const [typing, setTyping] = useState(false);
  const frame = useRef<HTMLDivElement>(null);
  useEffect(() => { if (typing) frame.current?.querySelector('input')?.focus(); }, [typing]);
  const menu = useRef<HTMLUListElement>(null);
  const total = groups.reduce((sum, group) => sum + group.rows.length, 0);
  const needle = fold(query.trim());
  const matching = (row: PickRow) => needle === '' || fold(`${row.name} ${row.place ?? ''} ${typeof row.detail === 'string' ? row.detail : ''} ${row.keywords ?? ''}`).includes(needle);
  // In each group the rows that can be chosen come first; the others keep their order after them.
  const shown = groups
    .map((group) => {
      const rows = group.rows.filter(matching);
      return { ...group, rows: [...rows.filter((row) => row.disabled !== true), ...rows.filter((row) => row.disabled === true)] };
    })
    .filter((group) => group.rows.length > 0);
  const rows = shown.flatMap((group) => group.rows);
  // One tab stop: the row that is chosen now, or the first that can be chosen.
  const stop = rows.find((row) => row.id === focusKey && row.disabled !== true)?.id
    ?? rows.find((row) => row.checked === true && row.disabled !== true)?.id ?? rows.find((row) => row.disabled !== true)?.id ?? null;
  // A header names a group among others: a single group needs none.
  const headers = shown.length > 1;
  return <div ref={frame} className={cx('flex min-h-0 w-72 max-w-full flex-col gap-2', className)} onKeyDownCapture={(event) => {
    if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey || isComposing(event.nativeEvent) || event.key.length !== 1 || event.key.trim() === '') return;
    if (!(event.target instanceof HTMLElement) || event.target.closest('[role="menu"]') === null) return;
    event.preventDefault(); event.stopPropagation(); setQuery((before) => before + event.key); setTyping(true);
  }}>
    {(total > SEARCH_FROM || typing) && <Input label={searchLabel ?? t('composer.pick.find')} hideLabel placeholder={searchLabel ?? t('composer.pick.find')} value={query} autoComplete="off"
      onChange={(event) => setQuery(event.target.value)}
      onKeyDown={(event) => {
        if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey || isComposing(event.nativeEvent) || !['ArrowDown', 'ArrowUp'].includes(event.key)) return;
        const options = Array.from(menu.current?.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]:not(:disabled)') ?? []);
        const target = event.key === 'ArrowUp' ? options.at(-1) : options.find((option) => option.tabIndex === 0) ?? options[0];
        if (target !== undefined) { event.preventDefault(); event.stopPropagation(); target.focus(); }
      }} />}
    {above}
    {total === 0 ? empty : rows.length === 0 ? <p role="status" className="px-2 text-sm text-text-muted">{t('composer.pick.nothing')}</p>
      : <ul ref={menu} role="menu" aria-label={label} aria-orientation="vertical" onKeyDown={moveFocus}
        // Where nothing in it can be chosen there is nothing to take the keyboard, and a list that scrolls has to be reached.
        {...(rows.every((row) => row.disabled === true) ? { tabIndex: 0 } : {})}
        className="flex max-h-(--list-max-height) min-h-0 w-full flex-col gap-1 overflow-y-auto">
        {shown.map((group) => <Fragment key={group.id}>
          {(headers || group.always === true) && group.label !== null && group.label !== '' && <li role="presentation" data-pick-group={group.id} className="px-2 pt-1 text-xs font-medium text-text-subtle">{group.label}</li>}
          {group.rows.map((row) => <li key={row.id} role="none" className="flex w-full flex-col">
            <Row row={row} tabStop={row.id === stop} onFocused={() => setFocusKey(row.id)} onChoose={() => onChoose(row)} />
            {row.action}
          </li>)}
        </Fragment>)}
      </ul>}
    {below}
    {hasChoiceDescription(facts) && <details className="group min-w-0 shrink-0" data-pick-facts>
      <summary className="focus-inset flex cursor-default list-none items-center gap-1 rounded-sm px-2 py-1 text-sm text-text-muted select-none hover:text-text [&::-webkit-details-marker]:hidden">
        <ChevronRight aria-hidden size={14} className="shrink-0 text-text-subtle transition-transform group-open:rotate-90" />{t('common.details')}
      </summary>
      <div className="min-w-0 pt-1">{facts}</div>
    </details>}
  </div>;
}

export interface ChoiceChipProps extends Omit<PickListProps, 'label' | 'className' | 'onChoose'> {
  /** A row was pressed; `close` shuts the list, which a row that opens something else in it leaves open. */
  onChoose: (row: PickRow, close: () => void) => void;
  /** What the control chooses ("Model", "Effort"): its name for the list and, with the value, for the button. */
  what: string;
  /** The current choice. */
  value: string;
  /** Show the subject beside values such as “None” that need their context. */
  showSubject?: boolean;
  /** Another name for the button's tooltip, such as the full value of a cut one. */
  title?: string;
  /** The control cannot be used, and says why on hover and focus. */
  reason?: string | undefined;
  disabled?: boolean;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** The side the list prefers: below, as a field's list opens; it opens above where the room below cannot show its rows (the box at the foot of the screen). */
  side?: 'top' | 'bottom';
  /** The name of the button where it is not "what: value". */
  ariaLabel?: string;
  /** Marks the control among the others of a row, for layout and for tests. */
  control?: string;
  className?: string;
}

/** A control that shows its current choice and opens its list. */
export function ChoiceChip({ what, value, showSubject = false, title, reason, disabled = false, open, onOpenChange, side = 'bottom', ariaLabel, control, className, onChoose, ...list }: ChoiceChipProps) {
  const { t } = useTranslation();
  const compact = useCompactControls();
  const [own, setOwn] = useState(false);
  const shown = open ?? own;
  const change = (next: boolean) => { setOwn(next); onOpenChange?.(next); };
  const name = t('composer.chip.value', { what, value });
  const trigger = <Button size="sm" variant="ghost" bounded disabled={disabled} reason={reason ?? title} aria-label={ariaLabel ?? name}
    className={cx('min-w-0 max-w-(--chip-width)', compact ? 'gap-1 px-1 text-xs' : 'gap-2 px-2 text-sm', className)}>
    <span className="min-w-0 truncate">{showSubject ? name : value === '' ? t('composer.chip.unset') : value}</span>
    <ChevronDown aria-hidden className={cx('size-4 shrink-0', compact && 'hidden')} />
  </Button>;
  return <span className="inline-flex min-w-0 max-w-full" data-composer-chip={control}>
    <Popover label={what} open={disabled ? false : shown} onOpenChange={change} side={side} scroll="children" className="max-w-[min(20rem,calc(100vw-2rem))]"
      trigger={trigger} onOpenAutoFocus={(event) => {
        const card = event.target;
        if (!(card instanceof HTMLElement)) return;
        const choice = card.querySelector<HTMLElement>('input:not(:disabled)')
          ?? card.querySelector<HTMLElement>('[role="menuitemradio"][tabindex="0"]:not(:disabled)')
          ?? card.querySelector<HTMLElement>('[role="menu"][tabindex="0"]');
        event.preventDefault();
        (choice ?? card).focus();
      }}>
      <PickList {...list} label={what} onChoose={(row) => onChoose(row, () => change(false))} />
    </Popover>
  </span>;
}
