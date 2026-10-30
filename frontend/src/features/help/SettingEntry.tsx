/** Every built-in setting has its own searchable help entry, including its schema, default, example and live source. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { targetSettingsQuery, type SettingsTarget } from '@/api/settings';
import { WhereSet } from '@/features/help/WhereSet';
import { GuideLink } from '@/features/help/GuideLink';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { authoredSettingGuide } from '@/features/help/settings';
import { ProviderGuideLink } from '@/features/help/ProviderGuideLink';
import { TemplateGuideLink } from '@/features/help/TemplateGuideLink';
import { SettingGuideLink } from '@/features/help/SettingGuideLink';
import { guidesForSetting } from '@/help/guidance';
import { providerGuidesForSetting } from '@/help/providers';
import { type Shape } from '@/features/settings/settingsModel';
import { settingFieldName, settingShape } from '@/features/settings/settingText';
import { SettingSummary } from '@/features/settings/SettingSummary';
import { settingChoice } from '@/i18n/choices';
import type { SettingDef } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { CopyButton } from '@/ui/CopyButton';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

function ValueDescription({ shape, settingKey }: { shape: Shape; settingKey: string }) {
  const { t } = useTranslation();
  if (shape.type === 'nullable') return <><ValueDescription shape={shape.of} settingKey={settingKey} /><p>{t('helpRelease.optionalValue')}</p></>;
  if (shape.type === 'choice') return <p>{t('helpRelease.choices', { values: shape.values.map((value)=>settingChoice(t,settingKey,value)?.name??value).join(t('startFlow.separator')) })}</p>;
  if (shape.type === 'int') return <div className="flex flex-col gap-1"><p>{t('helpRelease.integer')}</p>{shape.min !== null && <p>{t('helpRelease.minimum', { value: shape.min })}</p>}{shape.max !== null && <p>{t('helpRelease.maximum', { value: shape.max })}</p>}{shape.unit !== null && <p>{t('helpRelease.unit', { unit: t(`settings.editor.units.${shape.unit}`,{defaultValue:shape.unit}) })}</p>}</div>;
  if (shape.type === 'object') return <ul className="flex flex-col gap-2">{shape.members.map((member) => <li key={member.name}><span className="font-medium">{settingFieldName(member.name,settingKey,t)}{member.required ? ` (${t('helpRelease.required')})` : ''}</span><ValueDescription shape={member.shape} settingKey={settingKey} /></li>)}</ul>;
  if (shape.type === 'list' || shape.type === 'map') return <><p>{t(shape.type === 'list' ? 'helpRelease.list' : 'helpRelease.map')}</p><ValueDescription shape={shape.type === 'list' ? shape.item : shape.value} settingKey={settingKey} /></>;
  return <p>{t(`helpRelease.valueTypes.${shape.type}`)}</p>;
}

function CurrentSource({ target, def }: { target: SettingsTarget; def: SettingDef }) {
  const { t } = useTranslation();
  const values = useQuery({ ...targetSettingsQuery(target), staleTime: 0, refetchOnMount: 'always' });
  if (values.isPending || values.isFetching) return <SkeletonRows rows={2} />;
  if (values.isError) return <ErrorState compact error={values.error} onRetry={() => void values.refetch()} />;
  const setting = values.data.find((row) => row.key === def.key);
  if (setting === undefined) return <p className="text-sm text-text-muted">{t('helpRelease.sourceMissing')}</p>;
  return <WhereSet setting={setting} def={def} target={target} />;
}

export function SettingEntry({ def, target, onBack }: { def: SettingDef; target?: SettingsTarget; onBack: () => void }) {
  const { t } = useTranslation();
  const authored = authoredSettingGuide(def.key);
  return <article className="flex min-h-0 flex-1 flex-col gap-4 overflow-auto p-4" aria-label={def.title}>
    <Button variant="ghost" size="sm" className="self-start" onClick={onBack}>{t('helpRelease.backToSettings')}</Button>
    <h2 className="text-lg font-semibold text-text">{def.title}</h2>
    <p className="text-base text-text">{def.description}</p>
    <SettingGuideLink def={def} {...(target === undefined ? {} : {target})} />
    <div className="flex flex-wrap gap-2">{guidesForSetting(def.key).map((guide) => <GuideLink key={guide} guide={guide} named />)}</div>
    {authored !== null && <div className="flex"><ArticleGuideLink article={authored.article} heading={authored.heading} title={t(authored.title)} target={target} /></div>}
    <div className="flex flex-wrap gap-2">{providerGuidesForSetting(def.key).map((guide) => <ProviderGuideLink key={guide} guide={guide} named />)}</div>
    {(def.key === 'env.template_id' || def.key === 'runtime.template_id') && <div className="flex"><TemplateGuideLink /></div>}
    <CopyButton value={def.key} label={t('helpRelease.copyKey')} />
    <section className="flex flex-col gap-1"><h3 className="text-sm font-medium text-text">{t('helpRelease.values')}</h3><div className="text-sm text-text-muted"><ValueDescription shape={settingShape(def)} settingKey={def.key} /></div></section>
    <section className="flex flex-col gap-1"><h3 className="text-sm font-medium text-text">{t('helpRelease.shipped')}</h3><div className="text-sm">{def.default === null ? t('helpRelease.notFixed') : <SettingSummary def={def} value={def.default} />}</div></section>
    <section className="flex flex-col gap-1"><h3 className="text-sm font-medium text-text">{t('helpRelease.example')}</h3><div className="text-sm"><SettingSummary def={def} value={def.example} example /></div></section>
    {(def.key === 'agents.subagents_at_once' || def.key === 'agents.subagent_depth') && <p className="text-sm text-text-muted">{t('helpRelease.subagentExample')}</p>}
    {def.key === 'mcp.enabled' && <p className="text-sm text-text-muted">{t('helpRelease.mcpRestartExample')}</p>}
    {def.key === 'capability.recreate_grace_secs' && <p className="text-sm text-text-muted">{t('helpRelease.environmentRestartExample')}</p>}
    <p className="text-sm text-text-muted">{t('helpRelease.scopeAndMerge', { scopes: def.scopes.map((scope) => t(`settings.editor.detail.scope.${scope}`, { defaultValue: scope })).join(', '), merge: t(`settings.editor.detail.merges.${def.merge}`) })}</p>
    {target === undefined ? <p className="text-sm text-text-muted">{t('helpRelease.openFromControl')}</p> : <CurrentSource target={target} def={def} />}
  </article>;
}
