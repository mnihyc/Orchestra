/** Organization terms keep the registry's text writer, native joining order and bounded Markdown preview. */
import { useTranslation } from 'react-i18next';
import type { SettingsTarget } from '@/api/settings';
import type { ResolvedSetting } from '@/generated/api-types';
import { HelpHint } from '@/ui/HelpHint';
import { Markdown } from '@/ui/Markdown';
import { inheritedParts } from './settingParts';
import { ValueControl, type ValueControlProps } from './ValueControl';

export function OrgTermsPreview({ text }: { text: string }) {
  const { t } = useTranslation();
  return <HelpHint term={t('orgTerms.preview')} what={text === '' ? t('orgTerms.none') : <Markdown text={text} compact />} />;
}

export function OrgTermsControl({ setting, target, ...control }: ValueControlProps & { setting: ResolvedSetting; target: SettingsTarget }) {
  const pieces = inheritedParts(setting, target).filter((part) => !part.ignored).map((part) => part.value);
  const text = [...pieces, control.value].filter((value): value is string => typeof value === 'string')
    .map((value) => value.replace(/^[\r\n]+|[\r\n]+$/gu, '')).filter((value) => value !== '').join('\n\n');
  return <div className="flex w-full min-w-0 flex-col items-start gap-2">
    <ValueControl {...control} />
    <OrgTermsPreview text={setting.locked_by === null ? text : typeof setting.value === 'string' ? setting.value : ''} />
  </div>;
}
