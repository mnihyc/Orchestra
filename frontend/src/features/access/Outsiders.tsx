/** Somebody from outside the root org named on the page of a place, a resource or a document, by whoever holds the row for it or by an owner where the policy lets owners: a person, a role or a group; Chat only, a guest, or an invitation link; disabled, with who can, for everybody else.
 *
 * "Outside" means outside the whole tree of orgs: a colleague of another department is added like
 * anybody. The page itself says whether the caller may name an outsider (`external`). Where they may
 * not, the option is shown disabled, with the words that say who can: the holders of the row that
 * gives that. An address nobody is registered with is invited to this one place: the invitation
 * names what the person would hold, and its link is shown to copy.
 */
import { useMutation } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { Mail } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { type AccessTarget, isNodeKind } from '@/api/access';
import { createInvitation } from '@/api/users';
import type { AccessChoice, PermissionPage } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { CopyButton } from '@/ui/CopyButton';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';

import { holderName } from './holders';
import { type Catalogue, cellHolders } from './model';

/** The row that gives somebody the right to name an outsider, which belongs with no level. */
const EXTERNAL_ROW = 'access.external';

/** Whether a string is an address a person could be sent a message at. */
export const looksLikeAddress = (text: string): boolean => /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(text.trim());

/**
 * The words that say why the caller may not name an outsider here and who can, or `undefined` where
 * they may. Who can is read from the row that gives it: its own holders and the ones it follows.
 */
export function outsiderReason(t: TFunction, page: PermissionPage, target: Pick<AccessTarget, 'name'>): string | undefined {
  if (page.external) return undefined;
  const row = page.rows.find((entry) => entry.permission === EXTERNAL_ROW);
  const who = row === undefined ? [] : [...new Set(cellHolders(row).map((holder) => holderName(t, holder.holder, target.name)))];
  return who.length === 0 ? t('access.outsiders.nobody') : t('access.outsiders.whoCan', { who: who.join(', ') });
}

/** Invites an address nobody is registered with to this one place; shows the link to copy. */
export function InviteOutsider({
  target,
  choice,
  catalogue,
  email,
  open,
  onClose,
}: {
  target: AccessTarget;
  choice: AccessChoice;
  catalogue: Catalogue;
  email: string;
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const actKey = useActKey();
  const [address, setAddress] = useState(email);
  const permissions = catalogue.choice(choice, target.kind);
  const body = { kind: 'node', node_id: target.id, email: address.trim(), permissions } as const;
  const make = useMutation({ mutationFn: () => createInvitation(body, actKey(body)) });
  const made = make.data;
  const supported = isNodeKind(target.kind);

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !make.isPending && onClose()}
      size="sm"
      title={t('access.outsiders.inviteTitle', { name: target.name })}
      description={t('access.outsiders.inviteLead')}
      dismissible={!make.isPending}
      footer={
        made === undefined ? (
          <>
            <Button disabled={make.isPending} onClick={onClose}>
              {t('common.cancel')}
            </Button>
            <Button variant="primary" icon={<Mail aria-hidden />} pending={make.isPending} blocked={!supported || !looksLikeAddress(address)} onClick={() => make.mutate()}>
              {t('access.outsiders.inviteSend')}
            </Button>
          </>
        ) : (
          <Button variant="primary" onClick={onClose}>
            {t('common.close')}
          </Button>
        )
      }
    >
      <div className="flex flex-col gap-3 py-2">
        {made === undefined ? (
          <>
            <Input label={t('access.outsiders.address')} type="email" value={address} onChange={(event) => setAddress(event.target.value)} autoComplete="off" />
            <p className="text-sm text-text-muted">{t('access.outsiders.inviteWhat', { choice: catalogue.choiceLabel(choice), name: target.name })}</p>
            {!supported && <InlineAlert tone="info">{t('access.outsiders.inviteNodesOnly')}</InlineAlert>}
            {make.error !== null && <ErrorAlert error={make.error} />}
          </>
        ) : (
          <>
            <InlineAlert tone={made.sent ? 'success' : 'warning'}>
              {made.sent ? t('access.outsiders.sent', { email: address.trim() }) : t('access.outsiders.notSent', { email: address.trim() })}
            </InlineAlert>
            <div className="flex items-center gap-1">
              <Input label={t('access.outsiders.link')} hideLabel readOnly value={made.url} fieldClassName="min-w-0 flex-1" mono onFocus={(event) => event.currentTarget.select()} />
              <CopyButton value={made.url} label={t('access.outsiders.copyLink')} />
            </div>
          </>
        )}
      </div>
    </Dialog>
  );
}
