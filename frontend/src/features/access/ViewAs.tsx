/** What one person holds here, and through what.
 *
 * It is how "why can Chen not open a terminal?" is answered without reading rows: pick a person and the
 * page is shown as it is for them, each permission with what gives it to them: owning the thing, a
 * higher level of its area, or an entry (named on the page, or followed from a place's defaults).
 * Every permission-page kind uses the same effective-access request. A refused read keeps its cause
 * and Retry; it never turns into a claim that the person holds nothing.
 */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { type AccessTarget, effectiveQuery } from '@/api/access';
import type { AccessThrough, EffectiveAccess } from '@/generated/api-types';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { Combobox } from '@/ui/Combobox';
import { Dialog } from '@/ui/Dialog';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

import { useCandidates, useTargetOrg } from './candidates';
import { holderName, placeName } from './holders';
import { usePermissionName } from './names';

function Through({ through, thing }: { through: AccessThrough; thing: string }) {
  const { t } = useTranslation();
  const permissionName = usePermissionName();
  switch (through.kind) {
    case 'source_owner':
      return <>{t('access.sourceOwner')}</>;
    case 'owner':
      return <>{t('access.viewAs.owner')}</>;
    case 'admin':
      return <>{through.from === null ? t('access.viewAs.administers') : t('access.viewAs.adminFrom', { place: placeName(t, through.from) })}</>;
    case 'system_admin':
      return <>{t('access.viewAs.systemAdmin')}</>;
    case 'level':
      return <>{t('access.viewAs.level', { permission: permissionName(through.of) })}</>;
    case 'entry':
      return (
        <>
          {through.from === null
            ? t('access.viewAs.named', { holder: holderName(t, through.entry.holder, thing) })
            : t('access.viewAs.followed', { holder: holderName(t, through.entry.holder, thing), place: placeName(t, through.from) })}
          {(through.chain?.length ?? 0) > 1 && <ol aria-label={t('access.viewAs.chain')} className="mt-1 flex flex-col gap-1 text-text-muted">
            {through.chain?.map((holder, index) => <li key={index}>{t('access.viewAs.chainStep', { holder: holderName(t, holder, thing) })}</li>)}
          </ol>}
        </>
      );
  }
}

function Effective({ access, thing }: { access: EffectiveAccess; thing: string }) {
  const { t } = useTranslation();
  const permissionName = usePermissionName();
  return (
    <div className="flex flex-col gap-3">
      {(access.role !== null || access.administers) && <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        {access.role !== null && <Badge>{t(`roles.${access.role}`)}</Badge>}
        {access.administers && access.role !== 'admin' && <Badge tone="warning">{t('access.viewAs.administers')}</Badge>}
      </div>}
      {access.permissions.length === 0 ? (
        <EmptyState compact title={t('access.viewAs.nothing')} />
      ) : (
        <ul aria-label={t('access.viewAs.list')} className="flex flex-col divide-y divide-border rounded-lg border border-border">
          {access.permissions.map((entry) => (
            <li key={entry.permission} className="flex flex-col gap-1 px-3 py-2">
              <span className="text-sm font-medium text-text">{permissionName(entry.permission)}</span>
              <span className="text-sm text-text-muted">
                {entry.through.map((through, index) => (
                  <span key={index}>
                    {index > 0 && ' · '}
                    <Through through={through} thing={thing} />
                  </span>
                ))}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

export function ViewAs({ target, open, onClose }: { target: AccessTarget; open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const orgId = useTargetOrg(target, open);
  const [query, setQuery] = useState('');
  const [chosen, setChosen] = useState<string | null>(null);
  const candidates = useCandidates(query, orgId, { kinds: ['user'], thing: target.name, enabled: open });
  const access = useQuery({ ...effectiveQuery(target, chosen ?? ''), enabled: open && chosen !== null });
  const selectedUser = access.data?.user;

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && onClose()}
      size="md"
      title={t('access.viewAs.title', { name: target.name })}
      description={t('access.viewAs.lead')}
      footer={<Button onClick={onClose}>{t('common.close')}</Button>}
    >
      <div className="flex flex-col gap-4 py-2">
        <Combobox
          label={t('access.viewAs.person')}
          placeholder={t('access.viewAs.placeholder')}
          query={query}
          displayValue={selectedUser?.id === chosen ? selectedUser.display_name : undefined}
          onQueryChange={(next) => { setQuery(next); setChosen(null); }}
          minChars={2}
          loading={candidates.loading}
          empty={t('access.candidates.none')}
          groups={candidates.groups}
          onSelect={(option) => {
            const found = candidates.byValue.get(option.value);
            if (found?.input.kind !== 'user') return;
            setChosen(found.input.id);
            setQuery(found.name);
          }}
        />
        {chosen !== null &&
          (access.isPending ? (
            <SkeletonRows rows={4} />
          ) : access.isError ? (
            <ErrorState compact error={access.error} action={<Button size="sm" action="retry" pending={access.isRefetching} onClick={() => void access.refetch()}>{t('common.tryAgain')}</Button>} />
          ) : (
            <Effective access={access.data} thing={target.name} />
          ))}
      </div>
    </Dialog>
  );
}
