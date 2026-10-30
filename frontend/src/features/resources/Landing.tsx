/** Resource kinds, their authorized counts and their existing creation flows, from one summary read. */
import { Link, useNavigate } from '@tanstack/react-router';
import { Boxes } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { formatNumber } from '@/lib/format';
import { useAppearance } from '@/lib/appearance';
import { Page, PageHeader, PlaceMark } from '@/shell/Page';
import { useScreenHeading } from '@/shell/screen';
import { ErrorState } from '@/ui/ErrorState';
import { ResourceCreateButton } from './ResourceCreation';
import { KINDS } from './ResourceNav';
import { listAsLeft } from './trail';
import { useResourceSummary } from './summary';

export function ResourceLanding() {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const summary = useResourceSummary();
  const navigate = useNavigate();
  useScreenHeading({ title: t('screens.sections.resources'), icon: Boxes, trail: [] });
  return <Page>
    <PageHeader title={t('screens.sections.resources')} mark={<PlaceMark><Boxes aria-hidden /></PlaceMark>} />
    {summary.isError && <ErrorState compact error={summary.error} retrying={summary.isRefetching} onRetry={() => void summary.refetch()} />}
    <ul aria-label={t('resources.nav.label')} className="divide-y divide-border" data-resource-landing>
      {KINDS.map(({ name, to, icon: Icon }) => {
        const count = summary.data?.[name];
        const search = listAsLeft(name);
        return <li key={name} className="flex min-w-0 items-center gap-3 py-2" data-resource-kind={name}>
          <Link to={to} {...(search === undefined ? {} : { search })} className="focus-inset flex min-h-9 min-w-0 flex-1 items-center gap-3 rounded-md text-text hover:text-accent-text">
            <Icon aria-hidden className="shrink-0 text-text-subtle" /><span className="wrap-anywhere font-medium">{t(`screens.${name}.title`)}</span>
            <span className="ml-auto pr-1 text-sm text-text-muted tabular-nums" data-resource-count aria-busy={summary.isPending}>{count === undefined ? t('resources.landing.unread') : formatNumber(count, locale)}</span>
          </Link>
          <ResourceCreateButton kind={name} primary={false} compact onCreated={(id) => void navigate({ href: `${to}/${encodeURIComponent(id)}` })} />
        </li>;
      })}
    </ul>
  </Page>;
}
