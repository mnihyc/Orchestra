/** Shared SegmentedControl component (navigation); primitives are wrapped here.
 *
 * A choice among a few things that are all in view: a theme, a
 * density, a way of looking at a list. The arrow keys go from one to the next. Short names alone
 * determine the buttons' widths; descriptions open only in that option's hover/focus tooltip.
 * A choice not reported yet is null, leaving every segment unselected until it is known or chosen.
 */
import { RadioGroup } from 'radix-ui';
import { type ReactNode, useRef } from 'react';

import { ActionIcon, type ActionId } from '@/ui/actions';
import { Choice, type ChoiceDetails, type ChoiceName } from '@/ui/Choice';
import { cx } from '@/ui/cx';
import { Tooltip } from '@/ui/Tooltip';

export type Segment<Value extends string> = ChoiceName & ChoiceDetails & {
  value: Value;
  action?: ActionId;
  icon?: ReactNode;
  disabled?: boolean;
  /** Full contextual name when a short segment is all the row should show. */
  tooltip?: ReactNode;
};

export interface SegmentedControlProps<Value extends string> {
  /** What is being chosen, for a screen reader. */
  label: string;
  value: Value | null;
  onValueChange: (value: Value) => void;
  segments: readonly Segment<Value>[];
  /** A pending save holds the value and keeps keyboard focus on its segment. */
  pending?: boolean;
  size?: 'sm' | 'md';
  /** Gives the group the available width; options keep the width of their short names. */
  block?: boolean;
  /** Keep related labels on one line when the caller supplies a narrow alternative. */
  wrap?: boolean;
  /** Marks the chosen one as where the keyboard begins when a dialog that holds the control opens. */
  startsKeyboard?: boolean;
  className?: string;
}

export function SegmentedControl<Value extends string>({
  label,
  value,
  onValueChange,
  segments,
  pending = false,
  size = 'md',
  block = false,
  wrap = true,
  startsKeyboard = false,
  className,
}: SegmentedControlProps<Value>) {
  const group=useRef<HTMLDivElement>(null);
  return (
    <div data-choice-selector className={cx('inline-grid min-w-0 max-w-full self-start justify-items-start gap-1', block ? 'w-full' : 'w-fit', className)}>
    <RadioGroup.Root
      ref={group}
      data-segmented=""
      aria-label={label}
      aria-busy={pending || undefined}
      value={value ?? ''}
      // A real radio group selects on arrow navigation and never clears the chosen value.
      onValueChange={(next) => { if (!pending) onValueChange(next as Value); }}
      onKeyDownCapture={(event)=>{
        if(event.ctrlKey || event.altKey || event.metaKey)return;
        const target=event.target instanceof HTMLElement ? event.target.closest<HTMLElement>('[data-segment-value]') : null;
        const selectedValue=target?.dataset.segmentValue;
        if(target?.closest('[role="radiogroup"]')!==event.currentTarget)return;
        const available=segments.filter((segment)=>!segment.disabled);
        const index=available.findIndex((segment)=>segment.value===selectedValue);
        if(index<0)return;
        const rtl=event.currentTarget.dir==='rtl';
        const step=event.key==='ArrowRight' ? (rtl ? -1 : 1) : event.key==='ArrowLeft' ? (rtl ? 1 : -1) : event.key==='ArrowDown' ? 1 : event.key==='ArrowUp' ? -1 : null;
        const next=event.key==='Home' ? 0 : event.key==='End' ? available.length-1 : step===null ? null : (index+step+available.length)%available.length;
        const segment=next===null ? undefined : available[next];
        if(segment===undefined)return;
        // Select during the key press so a quick release cannot move focus without changing the choice.
        event.preventDefault();event.stopPropagation();
        if (pending) return;
        if(segment.value!==value)onValueChange(segment.value);
        requestAnimationFrame(()=>Array.from(group.current?.querySelectorAll<HTMLElement>('[data-segment-value]') ?? []).find((button)=>button.dataset.segmentValue===segment.value)?.focus());
      }}
      className={cx(
        'inline-flex max-w-full items-stretch gap-0.5 rounded-md bg-bg-muted p-0.5',
        wrap ? 'flex-wrap' : 'flex-nowrap',
        block && 'flex w-full',
      )}
    >
      {segments.map((segment) => (
        <Choice
          key={segment.value}
          name={segment.name ?? segment.label}
          description={segment.description}
          presentation="compact"
          compact
          className={cx('min-w-0 max-w-full', block && 'flex-1')}
        >{(content, control) => {
          const { 'aria-describedby': description, ...name } = control;
          return <Tooltip content={segment.tooltip ?? segment.description}><RadioGroup.Item
          {...name}
          {...(description === undefined ? {} : { 'aria-describedby': description })}
          value={segment.value}
          data-segment-value={segment.value}
          // A tooltip's open state must not replace the radio's selected state.
          data-state={value === segment.value ? 'checked' : 'unchecked'}
          disabled={segment.disabled}
          aria-disabled={pending || undefined}
          {...(startsKeyboard && segment.value === value ? { 'data-autofocus': '' } : {})}
          className={cx(
            'choice-option inline-flex min-w-0 flex-auto items-center justify-center gap-1 rounded-sm px-2 font-medium text-text-muted transition-colors',
            wrap ? 'whitespace-normal' : 'whitespace-nowrap',
            size === 'md' ? 'min-h-8 text-base' : 'min-h-6 text-sm',
            'hover:text-text',
            // System colours can erase the selected ground: its underline keeps a non-colour mark.
            'forced-colors:data-[state=checked]:underline forced-colors:data-[state=checked]:decoration-2 forced-colors:data-[state=checked]:underline-offset-4',
            'disabled:cursor-not-allowed disabled:text-text-disabled',
          )}
        >
          {segment.action === undefined ? segment.icon : <ActionIcon action={segment.action} />}
          {content}
        </RadioGroup.Item></Tooltip>;
        }}</Choice>
      ))}
    </RadioGroup.Root>
    </div>
  );
}
