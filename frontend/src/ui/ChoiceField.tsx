/** One question by a field's name explains its offered options, without adding captions to the page. */
import type { ReactNode } from 'react';
import { choiceName, hasChoiceDescription, type ChoiceDetails, type ChoiceName } from '@/ui/Choice';
import { HelpHint } from '@/ui/HelpHint';

type DescribedOption = ChoiceName & ChoiceDetails & { value?: string; note?: ReactNode; disabled?: ReactNode };
export interface ChoiceFieldHelpProps {
  label: string;
  options: readonly DescribedOption[];
  /** A consequence shared by all choices is explained once, outside their individual descriptions. */
  context?: ReactNode;
}

/** Use beside an existing legend/title, so that the field's name is never drawn twice. */
export function ChoiceFieldHelp({ label, options, context }: ChoiceFieldHelpProps) {
  const described = options.map((option) => ({ option, reason: typeof option.disabled === 'string' ? option.disabled : null }));
  return <HelpHint term={label} what={<div className="flex min-w-0 flex-col gap-3"><dl className="flex min-w-0 flex-col gap-3">
    {described.map(({ option, reason }, index) => <div key={option.value ?? index} className="min-w-0">
      <dt className="font-medium">{choiceName(option)}</dt>
      {hasChoiceDescription(option.description) && <dd className="text-sm text-text-muted">{option.description}</dd>}
      {hasChoiceDescription(option.note) && <dd className="text-sm text-text-muted">{option.note}</dd>}
      {reason !== null && <dd className="text-sm text-text-muted">{reason}</dd>}
    </div>)}
  </dl>{context !== undefined && <div className="text-sm text-text-muted">{context}</div>}</div>} />;
}

/** For fields without a visible title yet; controls still own their native label and keyboard behavior. */
export function ChoiceField({ label, options, context, children }: ChoiceFieldHelpProps & { children: ReactNode }) {
  return <div data-choice-field className="flex min-w-0 flex-col gap-2">
    <div className="inline-flex min-w-0 items-center gap-1 text-sm font-medium text-text">
      <span>{label}</span><ChoiceFieldHelp label={label} options={options} context={context} />
    </div>
    {children}
  </div>;
}
