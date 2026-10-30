/** The report uses the same authorized name locator as other person/place workflows. */
import { useTranslation } from 'react-i18next';
import { NamePicker } from '@/features/search/NamePicker';
import type { UsageScope } from '@/features/usage/search';
import type { ComponentProps } from 'react';

export interface ScopePickerProps {
  scope: { kind: UsageScope['kind'] | 'host' | 'system' };
  name: string | null;
  rootOrgId?: string;
  onChange: (scope: string) => void;
  kinds?: ComponentProps<typeof NamePicker>['kinds'];
  mineLabel?: string;
  placeholder?: string;
  nothing?: string;
}

export function ScopePicker({ scope, name, onChange, kinds = ['org', 'project'], mineLabel, placeholder, nothing }: ScopePickerProps) {
  const { t } = useTranslation();
  return <div className="w-[min(100%,16rem)] min-w-0 max-w-full">
    <NamePicker label={t('usage.scope.label')} kinds={kinds}
      chosen={scope.kind === 'me' ? null : name ?? t('common.loading')}
      anything={mineLabel ?? t('usage.scope.me.name')} placeholder={placeholder ?? t('usage.scope.search')} nothing={nothing ?? t('usage.scope.empty')}
      onChoose={(hit) => onChange(hit === null ? 'me' : `${hit.kind === 'host' ? 'host' : 'node'}:${hit.id}`)} />
  </div>;
}
