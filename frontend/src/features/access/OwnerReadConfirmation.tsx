/** The server's informed-sharing notice, with the same explicit confirmation on every share path. */
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { installOwnerReadReview, type SharingDisclosure } from '@/api/ownerReadConfirmation';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { InlineAlert } from '@/ui/InlineAlert';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';

export function OwnerReadNotice({ disclosure }: { disclosure: SharingDisclosure | null | undefined }) {
  const { t } = useTranslation();
  const {locale}=useFormatContext();
  return disclosure === null || disclosure === undefined ? null : <InlineAlert tone="warning"><p>{'hosts' in disclosure ? t('hosts.serverHost.grantNotice',{count:disclosure.hosts,number:formatNumber(disclosure.hosts,locale)}) : t('access.ownerReads.notice', { count: disclosure.count })}</p></InlineAlert>;
}

interface Request { disclosure: SharingDisclosure; answer: (confirmed: boolean) => void }

export function OwnerReadConfirmation() {
  const { t } = useTranslation();
  const queue = useRef<Request[]>([]);
  const [current, setCurrent] = useState<Request | null>(null);
  useEffect(() => {
    // The queue is one array for the component's life, changed in place; the effect and its cleanup share it.
    const pending = queue.current;
    const stop = () => { for (const request of pending.splice(0)) request.answer(false); setCurrent(null); };
    const uninstall = installOwnerReadReview((disclosure) => new Promise<boolean>((answer) => {
      const request = { disclosure, answer };
      pending.push(request);
      if (pending.length === 1) setCurrent(request);
    }), stop);
    return () => { uninstall(); for (const request of pending.splice(0)) request.answer(false); };
  }, []);
  const finish = (confirmed: boolean) => {
    const request = queue.current.shift();
    setCurrent(queue.current[0] ?? null);
    request?.answer(confirmed);
  };
  const serverHost = current !== null && 'hosts' in current.disclosure;
  return <ConfirmDialog open={current !== null} onOpenChange={(open) => { if (!open) finish(false); }} title={t(serverHost ? 'hosts.serverHost.grantTitle' : 'access.ownerReads.title')} confirmLabel={t(serverHost ? 'hosts.serverHost.grantConfirm' : 'access.ownerReads.confirm')} onConfirm={() => finish(true)}>
    <OwnerReadNotice disclosure={current?.disclosure} />
  </ConfirmDialog>;
}
