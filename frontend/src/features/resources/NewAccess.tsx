/** The "Who may use it" choice: names only, individual tooltips, one field question for all audiences.
 */
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import type { NewResourceAccess } from '@/generated/api-types';
import { ChoiceFieldHelp } from '@/ui/ChoiceField';
import { SegmentedControl } from '@/ui/SegmentedControl';

export interface NewAccessProps {
  value: NewResourceAccess;
  onChange: (value: NewResourceAccess) => void;
  /** The name of the org that would be its home. */
  home: string;
  /** Whether "everyone in the home" is a choice: it is not for an environment. */
  allowHome?: boolean;
  disabled?: boolean;
  label?: string;
}

export function NewAccess({ value, onChange, home, allowHome = true, disabled = false, label }: NewAccessProps) {
  const { t } = useTranslation();
  const segments = [
    { value: 'only_me' as const, ...choiceOption(t, 'resourceAccess.only_me', { home }), description: t('resources.access.gives.only_me', { home }) },
    ...(allowHome ? [{ value: 'home' as const, ...choiceOption(t, 'resourceAccess.home', { home }), description: t('resources.access.gives.home', { home }) }] : []),
    { value: 'defaults' as const, ...choiceOption(t, 'resourceAccess.defaults', { home }), description: t('resources.access.gives.defaults', { home }) },
  ];
  const context = t('resources.access.admins', { home });
  const caption = label ?? t('resources.access.label');
  return (
    <div className="flex flex-col gap-2">
      <span className="flex items-center gap-1 text-sm font-medium text-text">
        {caption}
        <ChoiceFieldHelp label={caption} options={segments} context={context} />
      </span>
      <SegmentedControl
        label={caption}
        value={value}
        onValueChange={onChange}
        segments={segments.map((segment) => ({ ...segment, disabled, description: <>{segment.description}{' '}{context}</> }))}
        className="flex-wrap"
      />
    </div>
  );
}
