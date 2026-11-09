/** A managed thing's sharing line opens the existing short access form. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { pageQuery, permissionsQuery, type AccessTarget } from '@/api/access';
import { Button } from '@/ui/Button';
import { openAccess } from './AccessDialog';
import { cardSummary } from './cardSummary';
import { generalAccessWords } from './GeneralAccess';
import { placeName } from './holders';
import { containerOf, generalAccessOf, indexCatalogue } from './model';

export function CardSharing({ target }: { target: AccessTarget }) {
  const { t } = useTranslation();
  const page = useQuery({ ...pageQuery(target), retry: false });
  const definitions = useQuery(permissionsQuery);
  if (page.isError || page.data === undefined || definitions.isError || definitions.data === undefined) return null;
  const catalogue = indexCatalogue(definitions.data);
  const summary = cardSummary(page.data, catalogue, target.kind);
  if (summary === null) return null;
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
