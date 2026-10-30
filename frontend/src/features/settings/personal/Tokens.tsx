import { ThreadStateIcon } from '@/ui/ThreadState';
/** Personal API-token scope and expiry.
 *
 * A token acts as its person, narrowed to what they choose (a page's preset or permissions by area,
 * as it opens; everything only when chosen) and, if they choose, to one place and what lies under
 * it, for as long as they say, with their fresh proof in the same act. Making one is the only moment
 * its secret is shown: the dialog keeps it on screen until it is closed and never asks the server for
 * it again; a creation answered again says it was made, with Revoke and make another. A token is
 * ended for good by revoking it, which asks once.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { KeyRound, Plus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { createToken, revokeToken, tokensQuery } from '@/api/users';
import { isPlainClick } from '@/features/chat/open';
import { PLACE_ICONS, placePath } from '@/features/navigation/places';
import { PersonalFrame } from '@/features/settings/PersonalFrame';
import { EVERYTHING, type Returned, returnedToTokenForm, TokenForm, TokenMade } from '@/features/settings/TokenForm';
import type { ApiTokenCreateBody, ApiTokenInfo, TokenCreated } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { useActKey } from '@/lib/forms';
import { formatNumber } from '@/lib/format';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';
import { ReferenceCard } from '@/ui/ReferenceCard';
import { SkeletonRows } from '@/ui/Skeleton';
import { Table, type Column } from '@/ui/Table';
import { toast } from '@/ui/Toast';

export { EVERYTHING, endOfToken, permissionsByArea } from '@/features/settings/TokenForm';

/** The form's id, which the dialog's footer submits. */
const FORM_ID = 'new-token';

function NewToken({ onClose, returned }: { onClose: () => void; returned: Returned | null }) {
  const { t } = useTranslation();
  const actKey = useActKey();
  const [made, setMade] = useState<TokenCreated | null>(null);
  const [revoking, setRevoking] = useState(false);
  const make = useMutation({
    mutationFn: (body: ApiTokenCreateBody) => {
      // The proof is not part of what the act is: a retry with a new code is the same creation.
      return createToken(body, actKey({ ...body, proof: null }));
    },
    onSuccess: (reply) => {
      setMade(reply);
    },
  });
  // A creation answered again has no secret: it is revoked, and the form opens again for another.
  const revoke = useMutation({
    mutationFn: (id: string) => revokeToken(id),
    onSuccess: () => {
      setMade(null);
      setRevoking(false);
      make.reset();
    },
  });
  const again = made !== null && made.token === null;
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !make.isPending && onClose()}
      title={made === null ? t('tokens.new.title') : again ? t('tokens.new.alreadyHeading') : t('tokens.new.madeTitle')}
      description={made === null ? t('tokens.new.lead') : again ? t('tokens.new.alreadyLead') : t('tokens.new.madeLead')}
      dismissible={!make.isPending}
      size="lg"
      footer={
        made === null ? (
          <>
            <Button disabled={make.isPending} onClick={onClose}>
              {t('common.cancel')}
            </Button>
            <Button type="submit" form={FORM_ID} variant="primary" pending={make.isPending}>
              {t('tokens.new.make')}
            </Button>
          </>
        ) : again ? (
          <>
            <Button onClick={onClose}>{t('common.close')}</Button>
            <Button variant="danger" pending={revoke.isPending} onClick={() => {
              setRevoking(true);
              revoke.mutate(made.info.id);
            }}>
              {t('tokens.new.revokeAndAgain')}
            </Button>
          </>
        ) : (
          <Button variant="primary" onClick={onClose}>
            {t('tokens.new.done')}
          </Button>
        )
      }
    >
      {made === null ? (
        <TokenForm formId={FORM_ID} onMake={(body) => make.mutate(body)} pending={make.isPending} error={make.error} returned={returned} />
      ) : (
        <>
          <TokenMade made={made} />
          {revoking && revoke.error !== null && <ErrorAlert error={revoke.error} />}
        </>
      )}
    </Dialog>
  );
}

function ScopesCell({ token, compact = false }: { token: ApiTokenInfo; compact?: boolean }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  if (token.scopes.includes(EVERYTHING)) return <span>{t('tokens.scopes.everything')}</span>;
  return (
    <details className={compact ? 'text-sm text-text-muted' : 'text-base'}>
      <summary className="cursor-default rounded-md select-none hover:text-text">{t('tokens.scopes.formattedCount', { count: token.scopes.length, amount: formatNumber(token.scopes.length, locale) })}</summary>
      <ul className="mt-1 flex flex-col gap-1 font-mono text-xs text-text-muted">
        {token.scopes.map((scope) => (
          <li key={scope}>{scope}</li>
        ))}
      </ul>
    </details>
  );
}

/** A token's native place name needs no additional read until its shared reference card opens. */
function TokenPlace({ node }: { node: ApiTokenInfo['node'] }) {
  const { t } = useTranslation();
  if (node === null) return <span>{t('tokens.everywhere')}</span>;
  if (node.redacted || node.name === null) return <span>{t('people.privateReference')}</span>;
  const Icon = PLACE_ICONS[node.kind];
  return <ReferenceCard reference={{ kind: 'node', id: node.id }} label={node.name}>
    {(keep) => <a href={placePath(node)} onClick={(event) => { if (isPlainClick(event)) { event.preventDefault(); keep(); } }}
      className="inline-flex min-w-0 max-w-full items-center gap-1 rounded-sm bg-bg-muted px-1 align-baseline font-medium text-text hover:bg-border">
      {node.kind === 'thread' ? <ThreadStateIcon display={node.thread_display} className="[&_svg]:size-4" /> : <Icon aria-hidden className="size-4 shrink-0 text-text-muted" />}<span className="truncate">{node.name}</span>
    </a>}
  </ReferenceCard>;
}

export function Tokens() {
  const { t } = useTranslation();
  const list = useQuery(tokensQuery);
  // Back from proving the linked sign-in again: the form opens as it was left, with the proof.
  const [returned] = useState(() => {
    const back = returnedToTokenForm();
    return back !== null && back.draft.serviceAccountId === undefined ? back : null;
  });
  const [making, setMaking] = useState(returned !== null);
  const [revoking, setRevoking] = useState<ApiTokenInfo | null>(null);
  const revoke = useMutation({
    mutationFn: (token: ApiTokenInfo) => revokeToken(token.id),
    onSuccess: (_none, token) => {
      setRevoking(null);
      toast({ tone: 'success', title: t('tokens.revoked', { name: token.name }) });
    },
  });

  const columns: Column<ApiTokenInfo>[] = [
    {
      id: 'name',
      header: t('tokens.columns.name'),
      cell: (token, shown) => {
        const scopesShown = shown.has('scopes'), placeShown = shown.has('where'), usedShown = shown.has('used'), expiryShown = shown.has('ends');
        return (
        <div className="flex min-w-0 flex-col gap-1">
          <div className="flex min-w-0 flex-wrap items-baseline gap-x-3 gap-y-1">
            <span className="font-medium wrap-anywhere text-text">{token.name}</span>
            {!scopesShown && <div className="text-sm text-text-muted"><ScopesCell token={token} compact /></div>}
          </div>
          <div className="flex flex-wrap gap-x-3 gap-y-1 text-sm text-text-muted">
            <span className="inline-flex flex-wrap gap-x-1"><span>{t('tokens.madePrefix')}</span><RelativeTime at={token.created_at} /></span>
            {!placeShown && <TokenPlace node={token.node} />}
            {!usedShown && <span>{token.last_used_at === null ? t('tokens.neverUsed') : <RelativeTime at={token.last_used_at} />}</span>}
            {!expiryShown && <span className="inline-flex flex-wrap gap-x-1"><span>{t('tokens.columns.ends')}</span><span>{token.expires_at === null ? t('tokens.neverEnds') : <RelativeTime at={token.expires_at} />}</span></span>}
          </div>
        </div>
        );
      },
    },
    { id: 'scopes', header: t('tokens.columns.mayDo'), from: 'sm', cell: (token) => <ScopesCell token={token} /> },
    { id: 'where', header: t('tokens.columns.where'), from: 'md', cell: (token) => <TokenPlace node={token.node} /> },
    {
      id: 'used',
      header: t('tokens.columns.lastUsed'),
      from: 'md',
      cell: (token) => (token.last_used_at === null ? <span className="text-text-muted">{t('tokens.neverUsed')}</span> : <RelativeTime at={token.last_used_at} />),
    },
    {
      id: 'ends',
      header: t('tokens.columns.ends'),
      from: 'lg',
      cell: (token) => (token.expires_at === null ? <span className="text-text-muted">{t('tokens.neverEnds')}</span> : <RelativeTime at={token.expires_at} />),
    },
    {
      id: 'revoke',
      header: t('tokens.revoke'),
      hideHeader: true,
      align: 'end',
      cell: (token) => (
        <Button size="sm" variant="ghost" onClick={() => setRevoking(token)}>
          {t('tokens.revoke')}
        </Button>
      ),
    },
  ];

  return (
    <PersonalFrame page="tokens" lead={t('tokens.lead')}>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="max-w-prose text-sm text-text-muted">{t('tokens.about')}</p>
        <Button variant="primary" icon={<Plus aria-hidden />} onClick={() => setMaking(true)}>
          {t('tokens.new.button')}
        </Button>
      </div>
      {list.isPending ? (
        <SkeletonRows rows={3} />
      ) : list.isError ? (
        <ErrorState error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
      ) : list.data.length === 0 ? (
        <EmptyState icon={<KeyRound aria-hidden />} title={t('tokens.none')} />
      ) : (
        <Table label={t('tokens.table')} columns={columns} rows={list.data} rowKey={(token) => token.id} />
      )}
      {making && <NewToken returned={returned} onClose={() => setMaking(false)} />}
      <ConfirmDialog
        open={revoking !== null}
        onOpenChange={(next) => !next && setRevoking(null)}
        title={t('tokens.revokeTitle', { name: revoking?.name ?? '' })}
        confirmLabel={t('tokens.revokeConfirm', { name: revoking?.name ?? '' })}
        danger
        pending={revoke.isPending}
        error={revoke.error ?? undefined}
        onConfirm={() => revoking !== null && revoke.mutate(revoking)}
      >
        {t('tokens.revokeBody')}
      </ConfirmDialog>
    </PersonalFrame>
  );
}
