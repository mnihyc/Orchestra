/** Request access to take a thing away as a file: whom to ask, a line for the reason, and where the request stands.
 *
 * Where somebody reads a thing and its Export row does not name them, Export… is switched off and this is the way
 * on: the right is asked for on the page that holds it, and the people who can give it are told why.
 */
import { useMutation } from '@tanstack/react-query';
import { Send } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { requestAccess } from '@/api/access';
import { useActKey } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Textarea } from '@/ui/Textarea';

import type { ExportTarget } from './target';

/** The longest reason a request takes. */
const REASON_MAX = 500;

/** The permission that lets a thing be taken away as a file, by the kind of the thing. */
const EXPORT_ROW = { thread: 'thread.export', chat: 'chat.export', document: 'doc.export' } as const;

export function RequestExport({ target, open, onClose }: { target: ExportTarget; open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const actKey = useActKey();
  const [reason, setReason] = useState('');
  const row = target.kind === 'thread' || target.kind === 'chat' || target.kind === 'document' ? EXPORT_ROW[target.kind] : null;
  const page = target.page ?? { kind: target.kind, id: target.id };
  const body = { items: row === null ? [] : [{ object: page, permissions: [row] }], reason: reason.trim() };
  const asking = target.owner?.display_name;

  const send = useMutation({ mutationFn: () => requestAccess(body, actKey(body)) });

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !send.isPending && onClose()}
      title={t('export.request.title', { name: target.name })}
      description={asking === undefined ? t('export.menu.notAllowedNoOwner') : t('export.menu.notAllowed', { name: asking })}
      size="sm"
      dismissible={!send.isPending}
      footer={
        <>
          <Button variant="secondary" disabled={send.isPending} onClick={onClose}>
            {send.isSuccess ? t('common.close') : t('common.cancel')}
          </Button>
          {!send.isSuccess && (
            <Button variant="primary" icon={<Send aria-hidden />} pending={send.isPending} blocked={row === null} onClick={() => row !== null && send.mutate()}>
              {t('export.request.send')}
            </Button>
          )}
        </>
      }
    >
      {send.isSuccess ? (
        <InlineAlert tone="success">{asking === undefined ? t('export.request.sentNoOwner') : t('export.request.sent', { name: asking })}</InlineAlert>
      ) : (
        <form
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            if (row !== null) send.mutate();
          }}
        >
          <Textarea
            label={t('export.request.reason')}
            hint={t('export.request.reasonHint', { max: REASON_MAX })}
            value={reason}
            maxLength={REASON_MAX}
            onChange={(event) => setReason(event.target.value)}
          />
          {send.error !== null && <ErrorAlert error={send.error} />}
        </form>
      )}
    </Dialog>
  );
}
