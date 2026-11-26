/** CA and machine certificate status.
 *
 * The authority every machine and every service of this system trusts: who it is, the span it is good
 * for and its fingerprint, which a person can compare with what a machine shows when it enrolls, and a
 * download of its certificate. Under it, the certificates of the system's own services and how many
 * machines hold one that ends within thirty days, with the first fifty named and linked.
 */
import { keepPreviousData, useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { Download, ShieldCheck } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useState } from 'react';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { pkiPageQuery } from '@/api/admin';
import { BASE_PATH } from '@/api/client';
import { daysLeft, groupFingerprint } from '@/features/admin/pki';
import { useAppearance } from '@/lib/appearance';
import { formatDateTime, useNow } from '@/lib/time';
import { boxClasses, Section } from '@/shell/Page';
import { StatusTag } from '@/features/admin/StatusTag';
import { buttonClasses } from '@/ui/Button';
import { CopyButton } from '@/ui/CopyButton';
import { cx } from '@/ui/cx';
import { ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { linkClasses } from '@/ui/link';
import { SkeletonRows } from '@/ui/Skeleton';
import { type Column, Table } from '@/ui/Table';

/** A service whose certificate ends within this many days is marked: a service renews itself when a third of its validity remains, so one this near its end has not been able to. */
const SOON_DAYS = 30;

export function Pki() {
  const search=useCollectionSearch('pki');
  return <PkiForReader key={search.scope} search={search}/>;
}
function PkiForReader({search}:{search:ReturnType<typeof useCollectionSearch>}) {
  const { t, i18n } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const now = useNow();
  const [path,setPath]=useState<{q:string;cursors:(string|undefined)[]}>({q:search.q,cursors:[undefined]});
  if(path.q!==search.q)setPath({q:search.q,cursors:[undefined]});
  const cursors=path.q===search.q?path.cursors:[undefined];
  const pki = useQuery({...pkiPageQuery(search.q,cursors.at(-1)),placeholderData:keepPreviousData});

  if (pki.isPending) return <SkeletonRows read={pki} rows={5} />;
  if (pki.isError) return <ErrorState error={pki.error} retrying={pki.isRefetching} onRetry={() => void pki.refetch()} />;

  const { ca, services, machines_expiring_30d: expiring, machines_expiring: machines } = pki.data;
  const when = (moment: string) => formatDateTime(moment, { locale, timeZone }, now);
  const fingerprint = groupFingerprint(ca.fingerprint_sha256);

  const columns: Column<(typeof services)[number]>[] = [
    { id: 'name', header: t('admin.pki.service'), cell: (service) => {
        const key = `admin.overview.parts.${service.name}`;
        return <span className="font-medium text-text">{i18n.exists(key) ? (t as (key: string) => string)(key) : service.name}</span>;
      },
    },
    { id: 'ends', header: t('admin.pki.ends'), cell: (service) => when(service.not_after) },
    {
      id: 'left',
      header: t('admin.pki.left'),
      from: 'sm',
      cell: (service) => {
        const days = daysLeft(service.not_after, now);
        return days <= SOON_DAYS ? <StatusTag tone="warning">{t('admin.pki.daysLeft', { count: days, number: days })}</StatusTag> : <span className="text-text-muted">{t('admin.pki.daysLeft', { count: days, number: days })}</span>;
      },
    },
  ];

  const machineColumns: Column<(typeof machines)[number]>[] = [
    {
      id: 'machine',
      header: t('admin.pki.machine'),
      cell: ({ machine }) => (
        <Link to="/resources/machines/$id" params={{ id: machine.id }} search={{ tab: 'certificate' }} className={linkClasses}>
          {machine.name}
        </Link>
      ),
    },
    {
      id: 'ends',
      header: t('admin.pki.ends'),
      cell: ({ not_after }) => <time dateTime={not_after}>{when(not_after)}</time>,
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <Section id="admin-ca" title={t('admin.pki.caTitle')} lead={t('admin.pki.caLead')}>
        <dl className={cx(boxClasses, 'grid grid-cols-[auto_1fr] gap-x-6 gap-y-3 p-4 text-base')}>
          <dt className="text-text-muted">{t('admin.pki.subject')}</dt>
          <dd className="min-w-0 font-mono text-sm wrap-anywhere text-text">{ca.subject}</dd>
          <dt className="text-text-muted">{t('admin.pki.validFrom')}</dt>
          <dd className="text-text">{when(ca.not_before)}</dd>
          <dt className="text-text-muted">{t('admin.pki.validUntil')}</dt>
          <dd className="text-text">{when(ca.not_after)}</dd>
          <dt className="text-text-muted">{t('admin.pki.fingerprint')}</dt>
          <dd className="flex min-w-0 items-start gap-1">
            <span className="min-w-0 font-mono text-sm leading-(--control-sm) wrap-anywhere text-text">{fingerprint}</span>
            <CopyButton value={ca.fingerprint_sha256} label={t('admin.pki.copyFingerprint')} />
          </dd>
        </dl>
        <div className="flex flex-wrap items-center gap-3">
          <a href={`${BASE_PATH}/pki/ca.pem`} download="orch-ca.pem" className={buttonClasses({ variant: 'primary' })}>
            <Download aria-hidden />
            {t('admin.pki.download')}
          </a>
          <span className="text-sm text-text-muted">{t('admin.pki.downloadHint')}</span>
        </div>
      </Section>

      <Section id="admin-services" title={t('admin.pki.servicesTitle')} lead={t('admin.pki.servicesLead')}>
        <Table label={t('admin.pki.servicesTitle')} columns={columns} rows={services} rowKey={(service) => service.name} />
      </Section>

      <Section id="admin-machines" title={t('admin.pki.machinesTitle')}>
        <CollectionSearch name={t('admin.pki.expiringMachines')} {...search}/>
        {expiring > 0 ? (
          <>
            <InlineAlert tone="warning" title={t('admin.pki.expiring', { count: expiring, number: expiring })}>
              {t('admin.pki.expiringHint')}
            </InlineAlert>
            <p className="text-base text-text-muted">{t('admin.pki.machinesLead')}</p>
            {machines.length===0?<EmptyState compact title={t('ui.collection.none')}/>:<Table label={t('admin.pki.expiringMachines')} columns={machineColumns} rows={machines} rowKey={({ machine }) => machine.id}/>}
            {(cursors.length>1 || pki.data.machines_next_cursor!==undefined && pki.data.machines_next_cursor!==null) && <div className="flex justify-end gap-2">
              <Button size="sm" disabled={cursors.length===1 || pki.isFetching || search.text.trim()!==search.q} onClick={()=>setPath({q:search.q,cursors:cursors.slice(0,-1)})}>{t('resourceDetails.layout.previous')}</Button>
              <Button size="sm" disabled={pki.data.machines_next_cursor===undefined || pki.data.machines_next_cursor===null || pki.isFetching || search.text.trim()!==search.q} onClick={()=>{const next=pki.data.machines_next_cursor;if(next!==undefined && next!==null)setPath({q:search.q,cursors:[...cursors,next]});}}>{t('ui.pagination.more')}</Button>
            </div>}
          </>
        ) : (
          <p className="flex items-center gap-2 text-base text-text">
            <ShieldCheck aria-hidden className="text-success" />
            {t('admin.pki.noneExpiring')}
          </p>
        )}
      </Section>
    </div>
  );
}
