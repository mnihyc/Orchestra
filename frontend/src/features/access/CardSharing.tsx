/** A managed thing's sharing line opens the existing short access form. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { pageQuery, permissionsQuery, type AccessTarget } from '@/api/access';
import { ApiError } from '@/api/client';
import { Button } from '@/ui/Button';
import { openAccess } from './AccessDialog';
import { cardSummary } from './cardSummary';
import { generalAccessWords } from './GeneralAccess';
import { placeName } from './holders';
import { containerOf, generalAccessOf, indexCatalogue } from './model';

export function CardSharing({ target }: { target: AccessTarget }) {
  const { t } = useTranslation();
  const page = useQuery({ ...pageQuery(target), retry: false });
  const definitions = useQuery({ ...permissionsQuery, retry: false });
  const writable = page.isSuccess && (page.data.owners_writable || page.data.rows.some((row) => row.writable));
  const error = page.isError ? page.error : definitions.isError ? definitions.error : null;
  const share = (reason?: string) => <Button size="sm" variant="ghost" blocked={!writable} reason={writable ? undefined : reason} onClick={() => { if (writable) openAccess(target, { share: true }); }}>{t('linkCards.share')}</Button>;
  const state = (label: string) => <div className="flex min-w-0 flex-wrap items-center gap-x-1 text-sm text-text-muted" data-card-sharing="">
    <span className="wrap-anywhere">{label}</span>
    {error !== null && <Button size="sm" variant="ghost" pending={page.isFetching || definitions.isFetching} onClick={() => { if (page.isError) void page.refetch(); if (definitions.isError) void definitions.refetch(); }}>{t('common.tryAgain')}</Button>}
    {share(label)}
  </div>;
  if (error !== null) return state(t(error instanceof ApiError && error.problem.status === 403 ? 'linkCards.sharingRestricted'
    : error instanceof ApiError && error.problem.status === 501 ? 'linkCards.sharingUnsupported' : 'linkCards.sharingUnavailable'));
  if (page.data === undefined || definitions.data === undefined) return state(t('linkCards.sharingChecking'));
  const catalogue = indexCatalogue(definitions.data);
  const summary = cardSummary(page.data, catalogue, target.kind);
  if (summary === null) return state(t('linkCards.sharingReadOnly'));
  const place = containerOf(page.data) ?? summary.from;
  const audienceId = target.kind === 'org' ? target.id : place !== null && place !== 'system' ? place.id : target.orgId ?? '';
  const audience = target.kind === 'org' ? target.name : placeName(t, place);
  const general = generalAccessOf(page.data, catalogue, target.kind, audienceId);
  const access = generalAccessWords(t, catalogue, target.kind, audience, general);
  return <div className="flex min-w-0 flex-wrap items-center gap-x-1 text-sm text-text-muted" data-card-sharing="">
    <span className="wrap-anywhere">{t('linkCards.sharing', { access })}</span>
    <span aria-hidden>·</span>
    <Button size="sm" variant="ghost" onClick={() => openAccess(target, { share: true })}>{t('linkCards.share')}</Button>
  </div>;
}
