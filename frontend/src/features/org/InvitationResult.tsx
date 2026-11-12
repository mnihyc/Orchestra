/** The same one-time capability result in org/system and outsider invitation dialogs. */
import { useMutation } from '@tanstack/react-query';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { revokeInvitation } from '@/api/users';
import type { InvitationCreateResponse } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { CopyButton } from '@/ui/CopyButton';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Input } from '@/ui/Input';
import { StateLine } from '@/ui/StateLine';

export function invitationWasCreated(reply: InvitationCreateResponse): boolean {
  return reply.already_created === true || typeof reply.url !== 'string';
}

export function InvitationResult({ reply, onAnother, linkLabel, copyLabel, hint, children }: {
  reply: InvitationCreateResponse;
  onAnother: () => void;
  linkLabel: string;
  copyLabel: string;
  hint?: string;
  children?: ReactNode;
}) {
  const { t } = useTranslation();
  const id = 'invitation' in reply ? reply.invitation.id : reply.invitation_id;
  const [confirming, setConfirming] = useState(false);
  const revoke = useMutation({ mutationFn: () => revokeInvitation(id), onSuccess: () => { setConfirming(false); onAnother(); } });
  if (invitationWasCreated(reply) || typeof reply.url !== 'string') {
    return <div className="flex min-w-0 flex-col gap-3">
      <StateLine compact role="status" title={t('places.invite.alreadyCreated')} action={
        <Button onClick={() => setConfirming(true)}>{t('places.invite.revokeAndAnother')}</Button>
      } />
      <ConfirmDialog open={confirming} onOpenChange={setConfirming} title={t('places.members.revokeTitle')} confirmLabel={t('places.members.revokeConfirm')} danger pending={revoke.isPending} error={revoke.error} onConfirm={() => revoke.mutate()}>
        {t('places.members.revokeBodyShared')}
      </ConfirmDialog>
    </div>;
  }
  return <>
    {children ?? <StateLine compact title={t('places.invite.created')} />}
    <div className="flex min-w-0 items-start gap-2">
      <Input label={linkLabel} hint={hint} value={reply.url} readOnly mono fieldClassName="min-w-0 flex-1" onFocus={(event) => event.currentTarget.select()} />
      <CopyButton value={reply.url} label={copyLabel} className="mt-6" />
    </div>
  </>;
}
