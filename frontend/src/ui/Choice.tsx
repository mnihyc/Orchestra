/** A choice shows the description its caller supplies; compact toolbar choices opt out explicitly. */
import { CircleQuestionMark } from 'lucide-react';
import { createContext, type ReactNode, useContext, useId } from 'react';

import { useGuideEntry } from '@/lib/guideEntries';
import { cx } from '@/ui/cx';
import type { CardPreview } from '@/ui/DetailCard';

export interface ChoiceHelp {
  /** The localized name of the guide button, including the choice it explains. */
  label: string;
  /** Open the actual guide, without choosing or changing this value. */
  onOpen: () => void;
  /** The actual destination, so a picker keeps one entrance to the same guide section. */
  guide?: { article: string; heading: string | null };
}

interface ChoiceGuideAttributes {
  'data-choice-help': true;
  'data-help-guide': string | undefined;
  'data-help-article': string | undefined;
  'data-help-heading': string | undefined;
}

/** `label` remains accepted while existing screens move their words into name/description pairs. */
export type ChoiceName<Name = ReactNode> =
  | { name: Name; label?: Name }
  | { name?: undefined; label: Name };

/** Keep a plain name plain for typeahead; legacy label is only a migration input. */
export function choiceName(choice: ChoiceName<string>): string;
export function choiceName(choice: ChoiceName): ReactNode;
export function choiceName(choice: ChoiceName): ReactNode { return choice.name ?? choice.label; }

export interface ChoiceDetails {
  description?: ReactNode;
  help?: ChoiceHelp;
  /** Details alongside the highlighted option, without changing its value. */
  preview?: CardPreview;
  /** A related chip remains outside the selectable control, with its own keyboard action. */
  accessory?: ReactNode;
}

export interface ChoiceControl {
  'aria-labelledby': string;
  'aria-describedby': string | undefined;
}

const PickerContext = createContext(false);

/** Only the contents of an open picker or tooltip inherit explanatory rows, never its trigger. */
export function ChoicePicker({ children }: { children: ReactNode }) {
  return <PickerContext value>{children}</PickerContext>;
}

export interface ChoiceProps {
  name: ReactNode;
  description?: ReactNode;
  accessory?: ReactNode;
  /** Forms and pickers explain choices; a toolbar explicitly keeps only the short name. */
  presentation?: 'compact' | 'picker' | 'form';
  /** Existing help slots are also accepted; new choices pass ChoiceHelp. */
  help?: ChoiceHelp | ReactNode;
  /** A stable base for the name and description, when an external control refers to them. */
  id?: string;
  /** Makes the words a real label for an adjacent checkbox, without putting help inside the label. */
  htmlFor?: string;
  compact?: boolean;
  className?: string;
  nameClassName?: string;
  /** Selectable controls receive the same words and explicit name/description associations. */
  children?: (content: ReactNode, control: ChoiceControl) => ReactNode;
  /** A menu gives the guide its own menu item and its existing roving keyboard behavior. */
  renderHelp?: (help: ChoiceHelp, mark: ReactNode, attributes: ChoiceGuideAttributes) => ReactNode;
}

function guide(value: ChoiceProps['help']): value is ChoiceHelp {
  return typeof value === 'object' && value !== null && 'label' in value && typeof value.label === 'string' && 'onOpen' in value && typeof value.onOpen === 'function';
}

export function hasChoiceDescription(description: ReactNode): boolean {
  return description !== undefined && description !== null && description !== false && description !== '';
}

export function Choice({ name, description, accessory, presentation, help, id, htmlFor, compact = false, className, nameClassName, children, renderHelp }: ChoiceProps) {
  const generated = useId();
  const inPicker = useContext(PickerContext);
  const mode = presentation ?? (inPicker ? 'picker' : 'form');
  const explains = mode !== 'compact';
  const entry = explains && guide(help) ? help : null;
  const { ref, hidden } = useGuideEntry(entry?.guide?.article ?? null, entry?.guide?.heading ?? null);
  const attributes: ChoiceGuideAttributes = {
    'data-choice-help': true,
    'data-help-guide': entry?.guide?.article,
    'data-help-article': entry?.guide?.article,
    'data-help-heading': entry?.guide?.heading ?? undefined,
  };
  const base = id ?? generated;
  const hasDescription = hasChoiceDescription(description) && explains;
  const control: ChoiceControl = { 'aria-labelledby': `${base}-name`, 'aria-describedby': hasDescription ? `${base}-description` : undefined };
  const words = <>
    <span data-choice-name id={control['aria-labelledby']} className={cx('break-words', nameClassName)}>{name}</span>
    {hasDescription && <span data-choice-description id={control['aria-describedby']} className={cx('font-normal whitespace-normal break-words text-text-muted', compact ? 'text-xs' : 'text-sm')}>{description}</span>}
  </>;
  const content = htmlFor === undefined
    ? <span className="flex min-w-0 flex-auto flex-col gap-0.5 text-left">{words}</span>
    : <label htmlFor={htmlFor} className="flex min-w-0 flex-auto flex-col gap-0.5 text-left">{words}</label>;
  const mark = <CircleQuestionMark size={14} aria-hidden />;
  return <span ref={ref} data-choice-layout={mode} className={cx('flex min-w-0 items-center gap-1.5', children !== undefined && 'choice-interactive', className)}>
    {children === undefined ? content : children(content, control)}
    {accessory !== undefined && <span data-choice-accessory className="inline-flex min-w-0 max-w-32 shrink-0">{accessory}</span>}
    {explains && (guide(help) ? hidden ? null : renderHelp === undefined
      ? <button type="button" {...attributes} aria-label={help.label} title={help.label} onClick={help.onOpen}
          className="focus-inset inline-flex size-6 shrink-0 items-center justify-center rounded-full text-text-subtle hover:bg-bg-muted hover:text-text">{mark}</button>
      : renderHelp(help, mark, attributes) : help)}
  </span>;
}

