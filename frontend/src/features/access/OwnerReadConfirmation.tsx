/** The server's informed-sharing notice, with the same explicit confirmation on every share path. */
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { installOwnerReadReview } from '@/api/ownerReadConfirmation';
import type { OwnerReadDisclosure } from '@/generated/api-types';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { InlineAlert } from '@/ui/InlineAlert';

export function OwnerReadNotice({ disclosure }: { disclosure: OwnerReadDisclosure | null | undefined }) {
  const { t } = useTranslation();
  return disclosure === null || disclosure === undefined ? null : <InlineAlert tone="warning"><p>{t('access.ownerReads.notice', { count: disclosure.count })}</p></InlineAlert>;
}

interface Request { disclosure: OwnerReadDisclosure; answer: (confirmed: boolean) => void }

export function OwnerReadConfirmation() {
  const { t } = useTranslation();
  const queue = useRef<Request[]>([]);
  const [current, setCurrent] = useState<Request | null>(null);
  useEffect(() => {
    const stop = () => { for (const request of queue.current.splice(0)) request.answer(false); setCurrent(null); };
    const uninstall = installOwnerReadReview((disclosure) => new Promise<boolean>((answer) => {
      const request = { disclosure, answer };
      queue.current.push(request);
      if (queue.current.length === 1) setCurrent(request);
    }), stop);
    return () => { uninstall(); for (const request of queue.current.splice(0)) request.answer(false); };
  }, []);
  const finish = (confirmed: boolean) => {
    const request = queue.current.shift();
    setCurrent(queue.current[0] ?? null);
    request?.answer(confirmed);
  };
  return <ConfirmDialog open={current !== null} onOpenChange={(open) => { if (!open) finish(false); }} title={t('access.ownerReads.title')} confirmLabel={t('access.ownerReads.confirm')} onConfirm={() => finish(true)}>
    <OwnerReadNotice disclosure={current?.disclosure} />
  </ConfirmDialog>;
}
