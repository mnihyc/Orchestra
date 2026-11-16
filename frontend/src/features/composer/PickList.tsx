/** The one list that the composer's choices share (model, effort, speed, access program, provider, agent): single-line rows with a muted second line only where there is something to say, the current row checked, groups under small headers that are shown only when they hold rows, a search field once more than eight rows are listed, and its own scroll.
 *
 * A control of the box shows its current value alone. Its list opens above it: nothing in it is chosen by
 * moving through it, a row is chosen by pressing it, and a row that cannot be chosen stays in its place,
 * greyed, with the reason in its second line. The same list stands in the dialogs that choose a provider,
 * an agent, a model or an effort, so a person meets one way of choosing.
 */
import { Check, ChevronDown } from 'lucide-react';
import { Fragment, type KeyboardEvent, type ReactNode, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useCompactControls } from '@/features/workbench/Narrow';
import { isComposing } from '@/lib/shortcuts';
import { Button } from '@/ui/Button';
import { Choice } from '@/ui/Choice';
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
  /** The muted second line. */
  detail?: ReactNode;
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
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const disabled = row.disabled === true;
  const name = <span className="flex min-w-0 items-center gap-2">
    <span className="min-w-0 truncate">{row.name}</span>
    {row.place !== undefined && row.place !== '' && <span className="min-w-0 truncate text-xs font-normal text-text-muted">{row.place}</span>}
    {row.mark}
  </span>;
  return <Choice presentation="picker" compact name={name} description={row.detail} className={cx('w-full', disabled ? 'text-text-disabled' : 'text-text')}>
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
  /** Said where the list has no row at all. */
  empty?: ReactNode;
  className?: string;
}

/** The rows of a choice, grouped, searchable and with their own scroll. */
export function PickList({ label, searchLabel, groups, onChoose, above, below, empty, className }: PickListProps) {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const [focusKey, setFocusKey] = useState<string | null>(null);
  const menu = useRef<HTMLUListElement>(null);
  const total = groups.reduce((sum, group) => sum + group.rows.length, 0);
  const needle = fold(query.trim());
  const shown = groups
    .map((group) => ({ ...group, rows: needle === '' ? group.rows : group.rows.filter((row) => fold(`${row.name} ${row.place ?? ''} ${row.keywords ?? ''}`).includes(needle)) }))
    .filter((group) => group.rows.length > 0);
  const rows = shown.flatMap((group) => group.rows);
  // One tab stop: the row that is chosen now, or the first that can be chosen.
  const stop = rows.find((row) => row.id === focusKey && row.disabled !== true)?.id
    ?? rows.find((row) => row.checked === true && row.disabled !== true)?.id ?? rows.find((row) => row.disabled !== true)?.id ?? null;
  // A header names a group among others: a single group needs none.
  const headers = shown.length > 1;
  return <div className={cx('flex min-h-0 w-72 max-w-full flex-col gap-2', className)}>
    {total > SEARCH_FROM && <Input label={searchLabel ?? t('composer.pick.find')} hideLabel placeholder={searchLabel ?? t('composer.pick.find')} value={query} autoComplete="off"
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
          </li>)}
        </Fragment>)}
      </ul>}
    {below}
  </div>;
}

export interface ChoiceChipProps extends Omit<PickListProps, 'label' | 'className' | 'onChoose'> {
  /** A row was pressed; `close` shuts the list, which a row that opens something else in it leaves open. */
  onChoose: (row: PickRow, close: () => void) => void;
  /** What the control chooses ("Model", "Effort"): its name for the list and, with the value, for the button. */
  what: string;
  /** What is chosen now: the only words on the button. */
  value: string;
  /** Another name for the button's tooltip, such as the full value of a cut one. */
  title?: string;
  /** The control cannot be used, and says why on hover and focus. */
  reason?: string | undefined;
  disabled?: boolean;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  side?: 'top' | 'bottom';
  /** The name of the button where it is not "what: value". */
  ariaLabel?: string;
  /** Marks the control among the others of a row, for layout and for tests. */
  control?: string;
  className?: string;
}

/** A control that shows its value alone and opens its list. */
export function ChoiceChip({ what, value, title, reason, disabled = false, open, onOpenChange, side = 'top', ariaLabel, control, className, onChoose, ...list }: ChoiceChipProps) {
  const { t } = useTranslation();
  const compact = useCompactControls();
  const [own, setOwn] = useState(false);
  const shown = open ?? own;
  const change = (next: boolean) => { setOwn(next); onOpenChange?.(next); };
  const name = t('composer.chip.value', { what, value });
  const trigger = <Button size="sm" variant="ghost" bounded disabled={disabled} reason={reason ?? title} aria-label={ariaLabel ?? name}
    className={cx('min-w-0 max-w-(--chip-width)', compact ? 'gap-1 px-1 text-xs' : 'gap-2 px-2 text-sm', className)}>
    <span className="min-w-0 truncate">{value === '' ? t('composer.chip.unset') : value}</span>
    <ChevronDown aria-hidden className={cx('size-4 shrink-0', compact && 'hidden')} />
  </Button>;
  return <span className="inline-flex min-w-0 max-w-full" data-composer-chip={control}>
    <Popover label={what} open={disabled ? false : shown} onOpenChange={change} side={side} scroll="children" className="max-w-[min(20rem,calc(100vw-2rem))]"
      trigger={trigger}>
      <PickList {...list} label={what} onChoose={(row) => onChoose(row, () => change(false))} />
    </Popover>
  </span>;
}
