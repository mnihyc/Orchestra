/** Ask to set what new threads start with: the right to manage the place, asked of the people who can give it, with a line for the reason and where the request stands.
 *
 * Where nothing says which provider, profile or model new threads of a place use, and the person may not write that
 * (it takes managing the place), the way on is to ask. The request names the place and the permission that is
 * missing, and goes to whoever can give it. The person sees the acknowledgment, or the refusal's cause
 * beside their retained reason so they can correct it or try the request again.
 */
import { useMutation } from '@tanstack/react-query';
import { Send } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { requestAccess } from '@/api/access';
import type { PlaceRef } from '@/features/navigation/places';
import type { UserRef } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Textarea } from '@/ui/Textarea';
import { RequestedEnd, RequestEndField, useRequestEnd } from '@/features/access/RequestEnd';

/** The longest reason a request takes. */
const REASON_MAX = 500;

export function RequestDefault({ place, owner, open, onClose }: { place: PlaceRef; owner: UserRef | null; open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const actKey = useActKey();
  const [reason, setReason] = useState('');
  const end = useRequestEnd();
  // What is written about how threads are bound is a parameter of the place, and its manager's.
  const body = { items: [{ object: { kind: place.kind, id: place.id }, permissions: [`${place.kind}.manage`] }], reason: reason.trim() };
  const asking = owner?.display_name;
  const send = useMutation({ mutationFn: () => {
    const expiresAt = end.end();
    const request = { ...body, ...(expiresAt === null ? {} : { expires_at: expiresAt }) };
    return requestAccess(request, actKey(request));
  } });

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !send.isPending && onClose()}
      title={t('start.request.title', { name: place.name })}
      description={asking === undefined ? t('start.request.about') : t('start.request.aboutAsk', { name: asking })}
      size="sm"
      dismissible={!send.isPending}
      footer={
        <>
          <Button variant="secondary" disabled={send.isPending} onClick={onClose}>
            {send.isSuccess ? t('common.close') : t('common.cancel')}
          </Button>
          {!send.isSuccess && (
            <Button variant="primary" icon={<Send aria-hidden />} pending={send.isPending} disabled={!end.valid} onClick={() => send.mutate()}>
              {t('start.request.send')}
            </Button>
          )}
        </>
      }
    >
      {send.isSuccess ? (
        <><InlineAlert tone="success">{asking === undefined ? t('start.request.sentNoOwner') : t('start.request.sent', { name: asking })}</InlineAlert>
          {send.data.approvals[0]?.kind === 'access' && <RequestedEnd at={send.data.approvals[0].request.expires_at} />}</>
      ) : (
        <form
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            if (end.valid) send.mutate();
          }}
        >
          <RequestEndField value={end} />
          <Textarea label={t('start.request.reason')} hint={t('start.request.reasonHint', { max: REASON_MAX })} value={reason} maxLength={REASON_MAX} onChange={(event) => setReason(event.target.value)} />
          {send.error !== null && <ErrorAlert error={send.error} />}
        </form>
      )}
    </Dialog>
  );
}
