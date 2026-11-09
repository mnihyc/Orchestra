/** One place picker, with native refusals and the native operation that still needs a choice. */
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useStore } from 'zustand';
import { useTranslation } from 'react-i18next';
import { ApiError, client } from '@/api/client';
import { copyDestinationsQuery, type CopyDestinationJudgement, type CopyIntent, type CopySubject } from '@/api/copyDestinations';
import { libraryReader } from '@/api/resourceLibrary';
import { openAccess } from '@/features/access/AccessDialog';
import { SettingHelpLink } from '@/features/help/SettingHint';
import { PlacePicker, type PickedPlace, type PlacePickerProps } from '@/features/settings/PlacePicker';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorState } from '@/ui/ErrorState';
import type { ChoiceHelp } from '@/ui/Choice';
import { StateLine } from '@/ui/StateLine';

export interface CopyPlacePickerProps extends Omit<PlacePickerProps, 'refusal' | 'onCandidatesChange' | 'optionHelp' | 'optionDescription'> {
  subject: CopySubject;
  sourceName?: string;
  intent: CopyIntent;
  onStanding: (judgement: CopyDestinationJudgement | null) => void;
  /** Same-root identity moves retain their existing native Manage operation. */
  homeMoveRoot?: string;
  homeMoveRefusal?: (place: PickedPlace) => string | false;
}
export function CopyPlacePicker({ subject, sourceName, intent, onStanding, homeMoveRoot, homeMoveRefusal, ...props }: CopyPlacePickerProps) {
  const { t } = useTranslation();
  const reader = useStore(client.session, () => JSON.stringify(libraryReader()));
  const [candidates, setCandidates] = useState<readonly PickedPlace[]>([]);
  const [reviewing, setReviewing] = useState<PickedPlace | null>(null);
  const receive = useCallback((next: readonly PickedPlace[]) => setCandidates((old) => JSON.stringify(old) === JSON.stringify(next) ? old : next), []);
  const nativeHome = useCallback((place: PickedPlace) => intent === 'move' && homeMoveRoot !== undefined && place.rootOrgId === homeMoveRoot, [intent, homeMoveRoot]);
  const selected = props.value;
  const destinations = useMemo(() => {
    const values = new Map(candidates.filter((place) => !nativeHome(place)).map((place) => [place.id, { kind: 'node' as const, id: place.id }]));
    if (selected !== null && !nativeHome(selected)) values.set(selected.id, { kind: 'node', id: selected.id });
    return [...values.values()];
  }, [candidates, selected, nativeHome]);
  const read = useQuery({ ...copyDestinationsQuery({ subject, intent, destinations }, libraryReader()), enabled: destinations.length > 0 });
  const rows = useMemo(() => new Map((read.data?.destinations ?? []).flatMap((row) => row.destination.kind === 'node' ? [[row.destination.id, row] as const] : [])), [read.data]);
  const standing = selected === null || nativeHome(selected) ? null : rows.get(selected.id) ?? null;
  useEffect(() => { onStanding(read.isSuccess && !read.isFetching ? standing : null); }, [standing, read.isSuccess, read.isFetching, onStanding, reader]);
  const description = (place: PickedPlace): string | undefined => {
    if (nativeHome(place)) return undefined;
    const outcome = rows.get(place.id)?.outcome;
    if (outcome?.state === 'needs_choice') return t(`duplicate.choices.${outcome.choice}`);
    return undefined;
  };
  const refusal = (place: PickedPlace): string | false => {
    if (nativeHome(place)) return homeMoveRefusal?.(place) ?? false;
    if (read.isError) return t('duplicate.judgeFailed');
    if (!read.isSuccess || read.isFetching) return t('duplicate.checking');
    const outcome = rows.get(place.id)?.outcome;
    if (outcome === undefined) return t('duplicate.checking');
    return outcome.state === 'blocked' ? t(`duplicate.causes.${outcome.cause}`) : false;
  };
  const help = (place: PickedPlace): ChoiceHelp | undefined => {
    const outcome = rows.get(place.id)?.outcome;
    return outcome === undefined || outcome.state === 'allowed' ? undefined
      : { label: t('duplicate.reviewDestination', { name: place.path }), onOpen: () => setReviewing(place) };
  };
  const details = reviewing === null ? undefined : rows.get(reviewing.id)?.outcome;
  const missing = details?.state === 'blocked' ? new ApiError(details.problem, null).extra('authz.denied')?.missing ?? [] : [];
  const policy = details?.state === 'blocked' && details.cause === 'source_policy';
  const at = policy ? details.problem.setting_at : undefined;
  const reviewAccess = () => {
    if (reviewing === null) return;
    const target = details?.state === 'blocked' && details.cause === 'destination_creation'
      ? { kind: reviewing.kind, id: reviewing.id, name: reviewing.path } : { ...subject, name: sourceName ?? t('access.missing.thisItem') };
    setReviewing(null);
    openAccess(target, { share: false, ...(missing[0]?.permissions[0] === undefined ? {} : { permission: missing[0].permissions[0] }) });
  };
  return <>
    <PlacePicker {...props} onCandidatesChange={receive} refusal={refusal} optionDescription={description} optionHelp={help} />
    {read.isError && <ErrorState compact error={read.error} onRetry={() => void read.refetch()} />}
    {reviewing !== null && details !== undefined && <Dialog open title={reviewing.path} onOpenChange={(open) => { if (!open) setReviewing(null); }} footer={<Button onClick={() => setReviewing(null)}>{t('common.close')}</Button>}>
      {details.state === 'blocked' ? <div className="flex min-w-0 flex-col gap-3"><ErrorState compact error={new ApiError(details.problem, null)} message={t(`duplicate.causes.${details.cause}`)}
        action={policy ? <SettingHelpLink settingKey="sharing.allow_copy_out" {...(typeof at === 'string' ? { target: { kind: 'node', id: at } as const } : at === null ? { target: { kind: 'system' } as const } : {})} /> : <Button onClick={reviewAccess}>{t('duplicate.reviewAccess')}</Button>} /></div>
        : details.state === 'needs_choice' ? <StateLine compact message={t(`duplicate.choices.${details.choice}`)} /> : null}
    </Dialog>}
  </>;
}
