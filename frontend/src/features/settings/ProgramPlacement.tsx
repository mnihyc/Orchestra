/** Background work uses named existing placements; ordinary thread work keeps its actual placement. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { nodeQuery } from '@/api/nodes';
import type { SettingsTarget } from '@/api/settings';
import { isRecord } from '@/features/settings/draftChecks';
import { PlacePicker, type PickedPlace } from '@/features/settings/PlacePicker';
import { ThingSettingChoice } from '@/features/settings/ThingChoice';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export function ProgramPlacement({ backend, onChange, target, disabled }: {
  backend:Record<string,unknown>; onChange:(backend:Record<string,unknown>)=>void;
  target?:SettingsTarget | undefined; disabled:boolean;
}) {
  const {t} = useTranslation();
  const config = isRecord(backend.config) ? backend.config : {};
  const environment = typeof config.environment_id === 'string' ? config.environment_id : '';
  const topic = typeof config.helper_topic_id === 'string' ? config.helper_topic_id : '';
  const placeId = target?.kind === 'node' ? target.id : '';
  const source = useQuery({...nodeQuery(placeId),enabled:placeId !== '',retry:false});
  const helper = useQuery({...nodeQuery(topic),enabled:topic !== '',retry:false});
  const change = (key:string,value:string | null) => {
    const next = Object.fromEntries(Object.entries(config).filter(([name])=>name !== key));
    if (value !== null && value !== '') next[key]=value;
    onChange({...backend,config:Object.keys(next).length === 0 ? null : next});
  };
  if (placeId !== '' && source.isPending) return <SkeletonRows reads={[source]} rows={1} />;
  if (source.isError) return <ErrorState compact error={source.error} onRetry={()=>void source.refetch()} />;
  if (source.data?.kind === 'thread') return <span className="text-xs text-text-muted">{t('settingsEditors.summarizer.threadPlacement')}</span>;
  const selected: PickedPlace | null = topic === '' ? null : {
    id:topic,kind:'topic',color:helper.data?.color ?? null,folder:false,
    path:helper.data?.name ?? t(helper.isPending ? 'common.loading' : 'settings.compact.managed'),
  };
  return <div className="flex min-w-0 flex-col gap-2">
    <ThingSettingChoice settingKey="env.default_id" shape={{type:'id'}} value={environment} label={t('settingsEditors.summarizer.environment')}
      disabled={disabled} onChange={(value)=>change('environment_id',typeof value === 'string' ? value : null)} />
    <PlacePicker label={t('settingsEditors.summarizer.helperTopic')} only="topic" value={selected} disabled={disabled}
      rootOrgId={source.data?.org_id} onChange={(value)=>change('helper_topic_id',value?.id ?? null)} />
    {helper.isError && <ErrorState compact error={helper.error} onRetry={()=>void helper.refetch()} />}
  </div>;
}
