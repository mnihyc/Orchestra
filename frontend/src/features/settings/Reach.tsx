/** The server counts actual contribution and readable overrides, including merge and lock semantics. */
import { useInfiniteQuery, useQueryClient, type InfiniteData } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { RefreshCw, X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { settingReachQuery, type SettingReachTarget } from '@/api/settings-reach';
import { usePlaceLabel } from '@/features/navigation/placeLabels';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import type { NodeRef, SettingDef, SettingReach } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { SkeletonRows } from '@/ui/Skeleton';

const KINDS = ['orgs', 'projects', 'topics', 'threads'] as const;

/** Directly on the row, with no initial request per key and no gear hiding its existence. */
export function SettingReachLine(props: { target: SettingReachTarget; def: Pick<SettingDef, 'key' | 'class'>; name: string; snapshot: string }) {
  const { t } = useTranslation();
  const [openedSnapshot, setOpenedSnapshot] = useState<string | null>(null);
  const open = openedSnapshot === props.snapshot;
  return <div data-setting-reach-line className="inline-flex min-w-0 max-w-full text-xs text-text-muted">
    {open ? <SettingReachDetails {...props} onClose={() => setOpenedSnapshot(null)} />
      : <Button size="sm" variant="ghost" aria-label={t('settings.reach.openFor', { name: props.name })} className="h-auto! min-h-6 px-0 py-0 text-xs" onClick={() => setOpenedSnapshot(props.snapshot)}>{t('settings.reach.open')}</Button>}
  </div>;
}
export function settingReachSentence(counts: SettingReach['applies'], t: TFunction, locale: string): string {
  const labels = KINDS.filter((kind) => counts[kind] > 0).map((kind) => t(`settings.reach.counts.${kind}`, { count: counts[kind] }));
  return labels.length === 0 ? t('settings.reach.none') : t('settings.reach.applies', { places: new Intl.ListFormat(locale).format(labels) });
}

export function SettingReachDetails({ target, def, name, snapshot, onClose }: { target: SettingReachTarget; def: Pick<SettingDef, 'key' | 'class'>; name?: string; snapshot: string; onClose?: () => void }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? 'en';
  const placeLabel = usePlaceLabel();
  const client = useQueryClient();
  const options = settingReachQuery(target, def.key, snapshot);
  const load = useInfiniteQuery(options);
  const refresh = () => {
    // Refreshing a previously expanded list starts from the first page, never replays every cursor.
    client.setQueryData<InfiniteData<SettingReach, string | null>>(options.queryKey, (held) => held === undefined ? held : {
      pages: held.pages.slice(0, 1), pageParams: held.pageParams.slice(0, 1),
    });
    void load.refetch();
  };
  const refused = load.error instanceof ApiError && [401, 403, 404].includes(load.error.status);
  const last = load.data?.pages.at(-1);
  const label = name === undefined ? t('settings.reach.label') : t('settings.reach.openFor', { name });
  const close = onClose === undefined ? null : <IconButton size="sm" icon={<X aria-hidden />} label={name === undefined ? t('settings.reach.close') : t('settings.reach.closeFor', { name })} onClick={onClose} />;
  if (last === undefined || refused) return <section aria-label={label} className="flex min-w-0 items-start gap-2">
    {load.isError ? <ErrorState compact error={load.error} retrying={load.isFetching} onRetry={refresh} /> : <SkeletonRows rows={2} />}{close}
  </section>;
  const overrides = [...new Map<string, NodeRef>((load.data?.pages ?? []).flatMap((page) => page.overrides.items).map((node) => [node.id, node] as const)).values()];
  return <section aria-label={label} className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1">
    <p>{settingReachSentence(last.applies, t, locale)}</p>
    {overrides.length > 0 && <div className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1"><span aria-hidden>·</span><span>{t('settings.reach.overridden')}</span>
      <ul className="flex min-w-0 flex-wrap gap-x-3 gap-y-1">{overrides.map((node) => <li key={node.id} className="min-w-0"><SourceSettingLink source={{ scope: node.kind, node }} def={def} target={target}
        fallback={t('access.place.hidden')} text={placeLabel(node) ?? t('access.place.hidden')} /></li>)}</ul></div>}
    {load.isError && <ErrorState compact error={load.error} retrying={load.isFetching} onRetry={load.isFetchNextPageError ? () => void load.fetchNextPage() : refresh} />}
    {load.hasNextPage && <Button size="sm" className="self-start" pending={load.isFetchingNextPage} onClick={() => void load.fetchNextPage()}>{t('settings.reach.more')}</Button>}
    <IconButton size="sm" icon={<RefreshCw aria-hidden />} label={name === undefined ? t('settings.reach.refresh') : t('settings.reach.refreshFor', { name })} pending={load.isFetching && !load.isFetchingNextPage} onClick={refresh} />
    {close}
  </section>;
}
