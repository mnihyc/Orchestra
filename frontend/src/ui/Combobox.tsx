/** Shared Combobox component (forms); primitives are wrapped here.
 *
 * One thing picked out of a list that follows what is typed: a person, a group, a role. The list is the
 * caller's (it may come from the server as the field is typed in), is drawn in groups under their
 * names, and stays in reach of the arrow keys even for what cannot be chosen, with the reason under
 * it. Enter chooses the one in front, Escape puts the list away, and the field keeps the keyboard
 * throughout.
 */
import { LoaderCircle } from 'lucide-react';
import { Popover as Primitive } from 'radix-ui';
import { usePromptCovered } from '@/ui/PromptScope';
import { type ComponentProps, type KeyboardEvent, type ReactNode, useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';

import { Choice, type ChoiceHelp } from '@/ui/Choice';
import { cx } from '@/ui/cx';
import { ChoiceCard, ChoiceInspection, type CardController, type CardPreview } from '@/ui/DetailCard';
import { EmptyState } from '@/ui/EmptyState';
import { Field, type FieldSlots } from '@/ui/Field';
import { fieldBoxClasses } from '@/ui/Input';
import { cardClasses, floatingAttachment, floatingClasses } from '@/ui/Popover';

export interface ComboboxOption<Value extends string> {
  value: Value;
  /** What the choice is, as it is drawn. */
  label: ReactNode;
  /** The same in plain text, for a screen reader. */
  text: string;
  /** The original complete name when the caller supplies a shortened label. */
  fullLabel?: string;
  /** One quiet line under the name. */
  description?: ReactNode;
  /** A mark before the name: an avatar, an icon. */
  icon?: ReactNode;
  /** Why it cannot be chosen; the option is then shown quietly with these words. */
  disabled?: string | boolean;
  /** Opens guidance without committing this option. */
  help?: ChoiceHelp;
  /** Inspects the highlighted option while the input keeps the keyboard. */
  preview?: CardPreview;
}

export interface ComboboxGroup<Value extends string> {
  id: string;
  /** Names the group above its options. */
  label: string;
  options: readonly ComboboxOption<Value>[];
}

export interface ComboboxProps<Value extends string> extends FieldSlots {
  /** An action inside the input's end, using the same placement as Input.trailing. */
  trailing?: ReactNode;
  /** Caller-owned paging/recovery actions; never selectable entity options. */
  footer?: ReactNode;
  /** What is typed. */
  query: string;
  onQueryChange: (query: string) => void;
  /** The chosen name while the list is closed; opening leaves the search free to type into. */
  displayValue?: string;
  /** Full text behind an explicitly shortened committed label, for the existing overflow tooltip. */
  displayFullValue?: string;
  /** Complete-choice fields start on the committed choice; typing still starts at the first match. */
  initialActiveValue?: Value;
  groups: readonly ComboboxGroup<Value>[];
  onSelect: (option: ComboboxOption<Value>) => void;
  /** Lazy readers run only while the input's suggestion list is open. */
  onOpenChange?: (open: boolean) => void;
  /** A complete-choice field can reopen after a refused write. Omitted keeps ordinary focus-owned behavior. */
  open?: boolean;
  /** Navigation can move focus to the chosen destination after the list has closed. */
  onCloseAutoFocus?: ComponentProps<typeof Primitive.Content>['onCloseAutoFocus'];
  /** The list is on its way. */
  loading?: boolean;
  /** What the list says when nothing matches what is typed. */
  empty?: ReactNode;
  /** The fewest characters typed before the list opens. */
  minChars?: number;
  placeholder?: string;
  disabled?: boolean;
  /** Match compact existing toolbar fields without changing ordinary form inputs. */
  size?: 'sm' | 'md';
  id?: string;
  fieldClassName?: string;
  className?: string;
}

/** Change only the measured popup viewport; no ancestor scrolling or React state mutation. */
function revealOption(viewport: HTMLElement, option: HTMLElement) {
  const frame = viewport.getBoundingClientRect(), row = option.getBoundingClientRect();
  // The existing rise-in animation scales the popup. Scroll offsets stay in unscaled CSS pixels.
  const scale = frame.height / viewport.offsetHeight;
  if (scale <= 0) return;
  const top = frame.top + viewport.clientTop * scale, bottom = top + viewport.clientHeight * scale;
  if (row.top < top) viewport.scrollTop += (row.top - top) / scale;
  else if (row.bottom > bottom) viewport.scrollTop += Math.min(row.top - top, row.bottom - bottom) / scale;
}

export function Combobox<Value extends string>({
  trailing,
  footer,
  query,
  onQueryChange,
  displayValue,
  displayFullValue,
  initialActiveValue,
  groups,
  onSelect,
  onOpenChange,
  open: requestedOpen,
  onCloseAutoFocus,
  loading = false,
  empty,
  minChars = 0,
  placeholder,
  disabled = false,
  size = 'md',
  label,
  optional,
  hint,
  error,
  hideLabel,
  aside,
  help,
  id,
  fieldClassName,
  className,
}: ComboboxProps<Value>) {
  const covered = usePromptCovered();
  const listId = useId();
  const [focused, setFocused] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const [active, setActive] = useState<number | null>(null);
  const [touchPreview, setTouchPreview] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const previewCard = useRef<CardController>(null);
  // State signals replacement; the mutable DOM node stays only in its ref.
  const [contentVersion, setContentVersion] = useState(0);
  const attachContent = useCallback((node: HTMLDivElement | null) => {
    if (content.current === node) return;
    content.current = node;
    setContentVersion((version) => version + 1);
  }, []);
  const restoringInput = useRef(false);
  const footerFocus = useRef(false);

  const flat = groups.flatMap((group) => group.options);
  const asked = query.trim().length >= minChars;
  const open = (requestedOpen ?? (focused && !dismissed)) && !disabled && asked;
  const optionId = (index: number) => `${listId}-option-${index}`;
  const initial = initialActiveValue === undefined ? 0 : Math.max(0, flat.findIndex((option) => option.value === initialActiveValue));
  const current = Math.min(active ?? initial, Math.max(0, flat.length - 1));
  const activeHelp = flat[current]?.help;
  const hasGuides = flat.some((option) => option.help !== undefined);
  const hasFooter = footer !== undefined && footer !== null && footer !== false;
  const hasPreviews = flat.some((option) => option.preview !== undefined);
  const hasControls = hasGuides || hasFooter || hasPreviews;
  const inPreview = (target: EventTarget | null) => target instanceof Element && target.closest('[data-choice-preview]')?.getAttribute('data-choice-preview') === listId;
  const guides = flat.flatMap((option, at) => {
    const help = option.help;
    return help !== undefined && (at === current || option.disabled === true || typeof option.disabled === 'string') ? [{ option, help }] : [];
  });

  const requestOpen = (next: boolean) => {
    setDismissed(!next);
    if (!next && initialActiveValue !== undefined) setActive(null);
    if (requestedOpen !== undefined) onOpenChange?.(next);
  };

  // Existing lazy readers observe focus-owned visibility. Controlled callers
  // receive explicit requests instead; echoing their props would close a retry
  // while its previous mutation is still pending.
  useEffect(() => {
    if (requestedOpen === undefined) onOpenChange?.(open && !covered);
  }, [requestedOpen, open, covered, onOpenChange]);

  useEffect(() => {
    if (requestedOpen === true && !disabled && !covered) input.current?.focus();
  }, [requestedOpen, disabled, covered]);

  const revealCurrent = useCallback(() => {
    if (!open || covered) return;
    const popup = content.current;
    if (popup === null) return;
    if (popup.ownerDocument.activeElement !== input.current) return;
    const list = popup.querySelector<HTMLElement>('[role="listbox"]');
    const option = list?.querySelector<HTMLElement>('[role="option"][aria-selected="true"]');
    const viewport = hasFooter ? list : popup;
    if (option === null || option === undefined || viewport === null || viewport.clientHeight === 0 || viewport.offsetHeight === 0) return;
    revealOption(viewport, option);
  }, [open, covered, hasFooter]);

  // Portal mount and Popper's available-height constraint settle after the input opens.
  // Observe the actual local viewport/list, including later wrapping or paging, without moving ancestors.
  useLayoutEffect(() => {
    if (!open || covered) return;
    const popup = content.current;
    if (popup === null) return;
    const list = popup.querySelector<HTMLElement>('[role="listbox"]');
    const viewport = hasFooter ? list : popup;
    if (list === null || viewport === null) return;
    let live = true;
    const reveal = () => { if (live) revealCurrent(); };
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(reveal);
    observer?.observe(viewport);
    if (list !== viewport) observer?.observe(list);
    for (const group of Array.from(list.children)) observer?.observe(group);
    reveal();
    return () => { live = false; observer?.disconnect(); };
  }, [open, covered, contentVersion, hasFooter, current, query, groups, revealCurrent]);

  const choose = (option: ComboboxOption<Value>) => {
    if (disabled || loading) return;
    if (option.disabled === true || typeof option.disabled === 'string') return;
    onSelect(option);
    requestOpen(false);
  };

  const openGuide = (guide: ChoiceHelp) => {
    requestOpen(false);
    guide.onOpen();
  };
  const guideKey = (event: KeyboardEvent<HTMLElement>): boolean => {
    if (!open || !hasControls) return false;
    if (event.key === 'F1' && activeHelp !== undefined) {
      event.preventDefault(); event.stopPropagation(); openGuide(activeHelp); return true;
    }
    if (event.key === 'Tab' && (guides.length > 0 || hasFooter || hasPreviews)) {
      // Paging and guidance keep their existing Tab order. Right Arrow always reaches inspection;
      // a picker without those controls also exposes the highlighted row's inspector through Tab.
      const buttons = Array.from(content.current?.querySelectorAll<HTMLElement>(hasFooter
        ? '[data-choice-help], [data-combobox-footer] button:not([disabled]), [data-combobox-footer] a[href], [data-combobox-footer] [tabindex="0"]'
        : guides.length > 0 ? '[data-choice-help]' : `[data-choice-for="${optionId(current)}"]`) ?? []);
      if (buttons.length === 0) return false;
      const at = buttons.findIndex((button) => button === event.target);
      const next = at < 0 ? event.shiftKey ? buttons.at(-1) : buttons[0] : buttons[at + (event.shiftKey ? -1 : 1)];
      event.preventDefault();
      if (next === undefined) input.current?.focus(); else next.focus();
      return true;
    }
    return false;
  };
  const press = (event: KeyboardEvent<HTMLInputElement>) => {
    if (['ArrowDown', 'ArrowUp', 'Home', 'End', 'ArrowRight'].includes(event.key)) setTouchPreview(false);
    if (guideKey(event)) return;
    switch (event.key) {
      case 'ArrowRight':
        if (open && query === '' && flat[current]?.preview !== undefined) { event.preventDefault(); previewCard.current?.keep(); }
        break;
      case 'ArrowDown':
        event.preventDefault();
        requestOpen(true);
        setActive(flat.length === 0 ? 0 : (current + 1) % flat.length);
        break;
      case 'ArrowUp':
        event.preventDefault();
        requestOpen(true);
        setActive(flat.length === 0 ? 0 : (current - 1 + flat.length) % flat.length);
        break;
      case 'Enter': {
        const option = flat[current];
        if (open) {
          event.preventDefault();
          if (option !== undefined) choose(option);
        }
        break;
      }
      case 'Home':
      case 'End':
        if (open) {
          event.preventDefault();
          setActive(event.key === 'Home' ? 0 : Math.max(0, flat.length - 1));
        }
        break;
      case 'Escape':
        if (open) {
          // The list goes, and not the dialog the field stands in.
          event.preventDefault();
          event.stopPropagation();
          requestOpen(false);
        }
        break;
      default:
    }
  };

  // Where each group begins in the one list the arrow keys go through.
  const starts = groups.map((_, at) => groups.slice(0, at).reduce((sum, earlier) => sum + earlier.options.length, 0));
  return (
    <Field label={label} optional={optional} hint={hint} error={error} hideLabel={hideLabel} aside={aside} help={help} className={fieldClassName} id={id}>
      {(control) => (
        <Primitive.Root open={open} onOpenChange={(next) => !next && requestOpen(false)}>
          <Primitive.Anchor data-floating-anchor asChild>
            <div className="relative">
              <input
                {...control}
                ref={input}
                type="text"
                role="combobox"
                aria-expanded={open}
                aria-controls={open ? hasControls ? `${listId}-popup` : listId : undefined}
                aria-autocomplete="list"
                aria-haspopup={hasControls ? 'dialog' : 'listbox'}
                aria-activedescendant={open && flat.length > 0 ? optionId(current) : undefined}
                aria-keyshortcuts={open && activeHelp !== undefined ? 'F1' : undefined}
                autoComplete="off"
                spellCheck={false}
                disabled={disabled}
                placeholder={placeholder}
                value={open ? query : displayValue ?? query}
                data-overflow-text={open ? undefined : displayFullValue}
                className={cx(fieldBoxClasses, size === 'sm' && 'h-(--control-sm) px-2 text-sm',
                  (loading || trailing !== undefined) && 'pr-10',
                  loading && trailing !== undefined && 'pr-[calc(var(--control-sm)+2.5rem)]', className)}
                onChange={(event) => {
                  onQueryChange(event.target.value);
                  requestOpen(true);
                  setActive(0);
                }}
                onFocus={() => {
                  footerFocus.current = false;
                  setFocused(true);
                  if (!restoringInput.current) requestOpen(true);
                  revealCurrent();
                }}
                onBlur={(event) => {
                  if (hasControls && (inPreview(event.relatedTarget) || event.relatedTarget instanceof Node && content.current?.contains(event.relatedTarget))) return;
                  setFocused(false);
                  if (requestedOpen !== undefined) requestOpen(false);
                }}
                onClick={() => requestOpen(true)}
                onPointerDown={(event) => setTouchPreview(event.pointerType === 'touch')}
                onKeyDown={press}
              />
              {loading && <LoaderCircle className={cx('absolute top-1/2 -translate-y-1/2 animate-spin text-text-subtle',
                // Shift the existing spinner slot by the trailing action's standard control width.
                trailing === undefined ? 'right-3' : 'right-[calc(var(--control-sm)+0.75rem)]')} aria-hidden />}
              {trailing !== undefined && <div className="absolute inset-y-0 right-1 flex items-center">{trailing}</div>}
            </div>
          </Primitive.Anchor>
          <Primitive.Portal>
            <Primitive.Content
              ref={attachContent}
              {...(hasControls ? { id: `${listId}-popup` } : {})}
              aria-label={hasControls && typeof label === 'string' ? label : undefined}
              style={covered ? { display: 'none' } : undefined}
              align="start"
              sideOffset={4}
              collisionPadding={8} {...floatingAttachment}
              // Options keep focus in the input; optional guide buttons are reached with Tab.
              onOpenAutoFocus={(event) => event.preventDefault()}
              onCloseAutoFocus={(event) => {
                onCloseAutoFocus?.(event);
                if (!event.defaultPrevented) {
                  event.preventDefault();
                  if (footerFocus.current) {
                    restoringInput.current = true; input.current?.focus(); restoringInput.current = false;
                  }
                }
                footerFocus.current = false;
              }}
              onFocusCapture={(event) => {
                if (hasFooter) footerFocus.current = event.target instanceof Element && event.target.closest('[data-combobox-footer]') !== null;
              }}
              onBlur={(event) => {
                if (!hasControls || inPreview(event.relatedTarget) || event.relatedTarget === input.current || event.relatedTarget instanceof Node && event.currentTarget.contains(event.relatedTarget)) return;
                if (event.relatedTarget instanceof Node) footerFocus.current = false;
                setFocused(false); requestOpen(false);
              }}
              onKeyDownCapture={(event) => {
                // Radix's looping FocusScope also handles Tab on this element.
                // Consume our explicit input/control cycle before its bubble handler.
                if (guideKey(event)) event.stopPropagation();
              }}
              onKeyDown={(event) => {
                if (hasControls && event.key === 'Escape') {
                  event.preventDefault(); event.stopPropagation(); requestOpen(false);
                  restoringInput.current = true; input.current?.focus(); restoringInput.current = false;
                }
              }}
              // A press on the field is the field's own; the list closes on a press anywhere else.
              onInteractOutside={(event) => {
                if (inPreview(event.target) || (event.target as Element | null)?.closest('input[role="combobox"]')) event.preventDefault();
                else footerFocus.current = false;
              }}
              className={cx(
                'relative z-(--layer-dropdown) [--floating-width:var(--menu-max-width)] w-(--menu-max-width) min-w-[min(var(--radix-popper-anchor-width),var(--menu-max-width),calc(100dvw-var(--spacing-4)))] animate-rise-in p-1 text-base text-text',
                floatingClasses,
                hasFooter ? 'flex flex-col overflow-hidden' : 'overflow-y-auto',
                cardClasses,
              )}
            >
              <ChoiceInspection><div id={listId} role="listbox" aria-label={typeof label === 'string' ? label : undefined} aria-busy={loading || undefined}
                className={hasFooter ? 'min-h-0 overflow-y-auto' : undefined}>
                {groups
                  .map((group, at) => group.options.length === 0 ? null : (

                    // A list of one unnamed group (a plain locator) has no heading row and no empty group name.
                    <div key={group.id} {...(group.label === '' ? {} : { role: 'group', 'aria-label': group.label })}>
                      {group.label !== '' && <div aria-hidden className="px-2 py-1 text-xs font-medium text-text-subtle">
                        {group.label}
                      </div>}
                      {group.options.map((option, position) => {
                        const mine = (starts[at] ?? 0) + position;
                        const off = option.disabled === true || typeof option.disabled === 'string';
                        return (
                          <Choice presentation="picker" key={option.value} name={option.preview?.renderName === undefined ? option.fullLabel === undefined ? option.label : <span data-overflow-text={option.fullLabel}>{option.label}</span> : option.preview.renderName(option.label)} description={typeof option.disabled === 'string' ? option.disabled : option.description} compact>
                          {(content, words) => <ChoiceCard preview={option.preview} active={mine === current && open && !touchPreview} controller={mine === current ? previewCard : undefined} owner={listId} returnFocus={() => { restoringInput.current = true; input.current?.focus(); restoringInput.current = false; }}><div
                            {...words}
                            id={optionId(mine)}
                            role="option"
                            aria-selected={mine === current}
                            aria-disabled={off || undefined}
                            // The pointer chooses on release, but the field must not lose the keyboard first.
                            onMouseDown={(event) => event.preventDefault()}
                            onClick={() => choose(option)}
                            onMouseMove={() => setActive(mine)}
                            onPointerMove={(event) => { if (event.pointerType !== 'touch') setTouchPreview(false); }}
                            className={cx(
                              'choice-option flex min-h-8 min-w-0 flex-1 cursor-default items-center gap-2 rounded-sm px-2 py-1 select-none',
                              mine === current && 'bg-bg-muted',
                              off ? 'text-text-muted' : 'text-text',
                            )}
                          >
                            {option.icon !== undefined && <span className="flex shrink-0 items-center text-text-muted">{option.icon}</span>}
                            {content}
                          </div></ChoiceCard>}
                          </Choice>
                        );
                      })}
                    </div>
                  ))}
                {flat.length === 0 && !loading && empty !== undefined && <EmptyState compact className="px-2" title={empty} />}
              </div>
              {guides.length > 0 && <div className={cx('flex flex-col gap-2 border-t border-border p-2', hasFooter && 'shrink-0')}>
                {guides.map(({ option, help: guide }) => <Choice presentation="picker" key={option.value} name={option.label} compact
                  help={{ ...guide, onOpen: () => openGuide(guide) }} />)}
              </div>}
              {hasFooter && <div data-combobox-footer className="shrink-0 border-t border-border p-2">{footer}</div>}
              </ChoiceInspection>
            </Primitive.Content>
          </Primitive.Portal>
        </Primitive.Root>
      )}
    </Field>
  );
}
