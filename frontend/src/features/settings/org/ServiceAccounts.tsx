/** Service identities and scoped tokens.
 *
 * An identity an org keeps for a program: it appears in lists with a service mark, is a member of
 * nothing until it is given a role, and acts through tokens that are made for it, shown once, and
 * revoked one by one. Whoever manages the members makes and removes them; removing one ends its tokens
 * and the roles and entries that named it, and the dialog says so before it asks.
 */
import { useInfiniteQuery, useMutation, useQuery } from '@tanstack/react-query';
import { Bot, KeyRound, Plus, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import {
  createServiceAccount,
  createServiceToken,
  deleteServiceAccount,
  orgTokensQuery,
  refuseOrgToken,
  revokeServiceToken,
  serviceAccountsQuery,
  serviceTokensQuery,
} from '@/api/directory';
import { takes } from '@/features/settings/refusals';
import { type Returned, returnedToTokenForm, TokenForm, TokenMade } from '@/features/settings/TokenForm';
import type { ApiTokenCreateBody, Org, OrgToken, ServiceAccount, TokenCreated } from '@/generated/api-types';
import { USERNAME_PATTERN } from '@/api/auth';
import { useFormatContext } from '@/lib/appearance';
import { useActKey } from '@/lib/forms';
import { holds } from '@/lib/permissions';
import { formatDateTime } from '@/lib/time';
import { boxClasses, Section } from '@/shell/Page';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Pagination } from '@/ui/Pagination';
import { PermissionGate } from '@/ui/PermissionGate';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';
import { UserChip } from '@/ui/UserChip';

function MakeAccount({ org, onClose }: { org: Org; onClose: () => void }) {
  const { t } = useTranslation();
  const actKey = useActKey();
  const [username, setUsername] = useState('');
  const [name, setName] = useState('');
  const [sent, setSent] = useState(false);
  const userWrong = USERNAME_PATTERN.test(username) ? undefined : t('settings.service.usernameRule');
  const nameWrong = name.trim() === '' ? t('settings.service.nameMissing') : undefined;
  const make = useMutation({
    mutationFn: () => {
      const body = { username, display_name: name.trim() };
      return createServiceAccount(org.id, body, actKey(body));
    },
    onSuccess: (account) => {
      toast({ tone: 'success', title: t('settings.service.made', { name: account.user.display_name }) });
      onClose();
    },
  });
  const submit = () => {
    setSent(true);
    if (userWrong === undefined && nameWrong === undefined && !make.isPending) make.mutate();
  };
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !make.isPending && onClose()}
      title={t('settings.service.newTitle', { org: org.name })}
      description={t('settings.service.newLead')}
      dismissible={!make.isPending}
      footer={
        <>
          <Button disabled={make.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" pending={make.isPending} onClick={submit}>
            {t('settings.service.make')}
          </Button>
        </>
      }
    >
      <form
        noValidate
        className="flex flex-col gap-4 py-2"
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <Input label={t('settings.service.name')} value={name} error={sent ? nameWrong : undefined} onChange={(event) => setName(event.target.value)} autoComplete="off" />
        <Input label={t('settings.service.username')} hint={t('settings.service.usernameHint')} mono value={username} error={sent ? userWrong : undefined} onChange={(event) => setUsername(event.target.value.toLowerCase())} autoComplete="off" />
        {make.error !== null && <ErrorAlert error={make.error} />}
      </form>
    </Dialog>
  );
}

/** The tokens of one identity, made for it with its maker's fresh proof and revoked one by one; a new one is shown once. */
function Tokens({ account, onClose, returned }: { account: ServiceAccount; onClose: () => void; returned: Returned | null }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const actKey = useActKey();
  const id = account.user.id;
  const list = useQuery(serviceTokensQuery(id));
  const [making, setMaking] = useState(returned !== null);
  const [made, setMade] = useState<TokenCreated | null>(null);
  const make = useMutation({
    // The proof is not part of what the act is: a retry with a new code is the same creation.
    mutationFn: (body: ApiTokenCreateBody) => createServiceToken(id, body, actKey({ ...body, proof: null })),
    onSuccess: (token) => {
      setMade(token);
      setMaking(false);
    },
  });
  const revoke = useMutation({
    mutationFn: (tokenId: string) => revokeServiceToken(id, tokenId),
  });
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !make.isPending && onClose()}
      title={t('settings.service.tokensTitle', { name: account.user.display_name })}
      description={t('settings.service.tokensLead')}
      size="lg"
      footer={
        making ? (
          <>
            <Button disabled={make.isPending} onClick={() => setMaking(false)}>
              {t('common.cancel')}
            </Button>
            <Button type="submit" form={SERVICE_TOKEN_FORM} variant="primary" pending={make.isPending}>
              {t('tokens.new.make')}
            </Button>
          </>
        ) : (
          <Button onClick={onClose}>{t('common.close')}</Button>
        )
      }
    >
      <div className="flex flex-col gap-4 py-2">
        {making ? (
          <TokenForm formId={SERVICE_TOKEN_FORM} serviceAccountId={id} onMake={(body) => make.mutate(body)} pending={make.isPending} error={make.error} returned={returned} />
        ) : (
          <div>
            <Button variant="primary" icon={<Plus aria-hidden />} onClick={() => {
              setMade(null);
              make.reset();
              setMaking(true);
            }}>
              {t('settings.service.makeToken')}
            </Button>
          </div>
        )}
        {made !== null && !making && <TokenMade made={made} />}
        {list.isPending ? (
          <SkeletonRows rows={2} />
        ) : list.isError ? (
          <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
        ) : list.data.length === 0 ? (
          <EmptyState compact title={t('settings.service.noTokens')} />
        ) : (
          <ul className={`${boxClasses} divide-y divide-border`}>
            {list.data.map((token) => (
              <li key={token.id} className="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-2">
                <KeyRound className="size-4 shrink-0 text-text-subtle" aria-hidden />
                <span className="min-w-0 flex-1 basis-[min(100%,10rem)]">
                  <span className="text-base font-medium wrap-anywhere text-text">{token.name}</span>
                  <span className="ml-3 text-sm text-text-muted">{t('tokens.madeOn', { time: formatDateTime(token.created_at, format) })}</span>
                </span>
                <span className="text-sm text-text-muted">{token.last_used_at === null ? t('tokens.neverUsed') : <RelativeTime at={token.last_used_at} />}</span>
                <Button size="sm" variant="danger-ghost" pending={revoke.isPending && revoke.variables === token.id} onClick={() => revoke.mutate(token.id)} aria-label={t('tokens.revokeTitle', { name: token.name })}>
                  {t('tokens.revoke')}
                </Button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </Dialog>
  );
}

/** The form's id, which the dialog's footer submits. */
const SERVICE_TOKEN_FORM = 'new-service-token';

/** Tokens that reach the org: whose, what each may do and where, when it was used and ends; Refuse here for this token in this org alone. Never a secret, and nobody's token is made from here. */
function OrgTokens({ org }: { org: Org }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const list = useInfiniteQuery(orgTokensQuery(org.id));
  const tokens = list.data?.pages.flatMap((page) => page.items) ?? [];
  const [refusing, setRefusing] = useState<OrgToken | null>(null);
  const refuse = useMutation({
    mutationFn: (token: OrgToken) => refuseOrgToken(org.id, token.id),
    onSuccess: (_none, token) => {
      toast({ tone: 'success', title: t('settings.orgTokens.refused', { name: token.name, org: org.name }) });
      setRefusing(null);
    },
  });
  return (
    <Section id="org-tokens" title={t('settings.orgTokens.title', { org: org.name })} lead={t('settings.orgTokens.lead', { org: org.name })}>
      {list.isPending ? (
        <SkeletonRows rows={2} />
      ) : list.isError && tokens.length === 0 ? (
        <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
      ) : tokens.length === 0 ? (
        <EmptyState compact title={t('settings.orgTokens.none', { org: org.name })} />
      ) : (
        <div className="flex flex-col gap-2">
          <ul className={`${boxClasses} divide-y divide-border`} aria-label={t('settings.orgTokens.title', { org: org.name })}>
            {tokens.map((token) => (
              <li key={token.id} className="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-2">
                <span className="min-w-0 flex-1 basis-[min(100%,14rem)]">
                  <span className="flex flex-wrap items-center gap-2">
                    <UserChip user={token.user} size={24} />
                    <span className="font-medium wrap-anywhere text-text">{token.name}</span>
                    {token.refused_here && <Badge tone="danger">{t('settings.orgTokens.refusedHere')}</Badge>}
                    {!token.lifetime_allowed && <Badge tone="warning">{t('settings.orgTokens.tooLong')}</Badge>}
                  </span>
                  <span className="block text-sm text-text-muted">
                    {token.scopes.includes('*') ? t('tokens.scopes.everything') : t('tokens.scopes.count', { count: token.scopes.length })}
                    {' · '}
                    {token.node === null ? t('tokens.everywhere') : token.node.name}
                    {' · '}
                    {token.expires_at === null ? t('tokens.neverEnds') : formatDateTime(token.expires_at, format)}
                  </span>
                </span>
                <span className="text-sm text-text-muted">{token.last_used_at === null ? t('tokens.neverUsed') : <RelativeTime at={token.last_used_at} />}</span>
                {!token.refused_here && (
                  <Button size="sm" variant="danger-ghost" onClick={() => setRefusing(token)}>
                    {t('settings.orgTokens.refuse')}
                  </Button>
                )}
              </li>
            ))}
          </ul>
          <Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} />
        </div>
      )}
      <ConfirmDialog
        open={refusing !== null}
        onOpenChange={(next) => !next && setRefusing(null)}
        title={t('settings.orgTokens.refuseTitle', { name: refusing?.name ?? '', org: org.name })}
        confirmLabel={t('settings.orgTokens.refuse')}
        danger
        pending={refuse.isPending}
        error={refuse.error ?? undefined}
        onConfirm={() => refusing !== null && refuse.mutate(refusing)}
      >
        {t('settings.orgTokens.refuseBody', { org: org.name })}
      </ConfirmDialog>
    </Section>
  );
}

export function ServiceAccounts({ org }: { org: Org }) {
  const { t } = useTranslation();
  const may = holds(org.access, 'members.manage');
  const refusal = takes(t, 'members.manage', org.name, org.owner);
  const list = useQuery({ ...serviceAccountsQuery(org.id), enabled: may });
  const [making, setMaking] = useState(false);
  const [tokens, setTokens] = useState<ServiceAccount | null>(null);
  // Back from proving the linked sign-in again for a service token: its identity's tokens open as they were left.
  const [returned] = useState(() => {
    const back = returnedToTokenForm();
    return back?.draft.serviceAccountId === undefined ? null : back;
  });
  const [closedReturn, setClosedReturn] = useState(false);
  const returnedTo = returned === null ? undefined : list.data?.find((account) => account.user.id === returned.draft.serviceAccountId);
  const shownTokens = tokens ?? (returnedTo !== undefined && !closedReturn ? returnedTo : null);
  const [removing, setRemoving] = useState<ServiceAccount | null>(null);
  const remove = useMutation({
    mutationFn: (account: ServiceAccount) => deleteServiceAccount(account.user.id),
    onSuccess: (_none, account) => {
      toast({ tone: 'success', title: t('settings.service.removed', { name: account.user.display_name }) });
      setRemoving(null);
    },
  });
  return (
    <Section
      id="service-accounts"
      title={t('settings.org.sectionNames.service-accounts')}
      lead={t('settings.service.lead', { name: org.name })}
      action={
        <PermissionGate allowed={may} mode="block" reason={refusal}>
          <Button size="sm" variant="primary" icon={<Plus aria-hidden />} onClick={() => setMaking(true)}>
            {t('settings.service.new')}
          </Button>
        </PermissionGate>
      }
    >
      {!may ? (
        <EmptyState compact icon={<Bot aria-hidden />} title={t('settings.service.readers', { name: org.name })} description={refusal} />
      ) : list.isPending ? (
        <SkeletonRows rows={2} />
      ) : list.isError ? (
        <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
      ) : list.data.length === 0 ? (
        <EmptyState compact icon={<Bot aria-hidden />} title={t('settings.service.none', { name: org.name })} description={t('settings.service.noneAbout')} />
      ) : (
        <ul className={`${boxClasses} divide-y divide-border`} aria-label={t('settings.service.list', { name: org.name })}>
          {list.data.map((account) => (
            <li key={account.user.id} className="flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3">
              <span className="min-w-0 flex-1 basis-[min(100%,12rem)]">
                <UserChip user={account.user} size={28} showUsername />
              </span>
              <span className="text-sm text-text-muted">{t('settings.service.tokenCount', { count: account.token_count })}</span>
              <Button size="sm" onClick={() => setTokens(account)} aria-label={t('settings.service.tokensOf', { name: account.user.display_name })}>
                {t('settings.service.tokensButton')}
              </Button>
              <Button size="sm" variant="danger-ghost" icon={<Trash2 aria-hidden />} onClick={() => setRemoving(account)} aria-label={t('settings.service.remove', { name: account.user.display_name })}>
                {t('settings.channels.remove')}
              </Button>
            </li>
          ))}
        </ul>
      )}
      {making && <MakeAccount org={org} onClose={() => setMaking(false)} />}
      {shownTokens !== null && (
        <Tokens
          account={shownTokens}
          returned={tokens === null ? returned : null}
          onClose={() => {
            setTokens(null);
            setClosedReturn(true);
          }}
        />
      )}
      <ConfirmDialog
        open={removing !== null}
        onOpenChange={(next) => !next && setRemoving(null)}
        title={t('settings.service.removeTitle', { name: removing?.user.display_name ?? '' })}
        confirmLabel={t('settings.service.removeConfirm', { name: removing?.user.display_name ?? '' })}
        danger
        pending={remove.isPending}
        error={remove.error ?? undefined}
        onConfirm={() => removing !== null && remove.mutate(removing)}
      >
        {t('settings.service.removeBody')}
      </ConfirmDialog>
      {may && <OrgTokens org={org} />}
    </Section>
  );
}
