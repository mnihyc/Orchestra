/** One local Markdown composer for both native administrator-message scopes. */
import { useInfiniteQuery, useMutation } from '@tanstack/react-query';
import { useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { sendMessage, sentMessagesQuery } from '@/api/adminMessages';
import { formatNumber } from '@/lib/format';
import { useActKey } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { DateField } from '@/ui/DateField';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Markdown } from '@/ui/Markdown';
import { StateLine } from '@/ui/StateLine';
import { Tabs } from '@/ui/Tabs';
import { Textarea } from '@/ui/Textarea';
import { Audience } from './Audience';
import { messageReadRefused } from './current';

export type MessageScope = { kind: 'org'; id: string; name: string } | { kind: 'system' };

export function MessageComposer({ scope, initialTitle = '', unavailableReason, onClose }: { scope: MessageScope; initialTitle?: string; unavailableReason?: string | undefined; onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const org = scope.kind === 'org' ? scope.id : null;
  const list = useInfiniteQuery(sentMessagesQuery(org));
  const count = unavailableReason !== undefined || messageReadRefused(list.error) ? undefined : list.data?.pages[0]?.audience_count;
  const [title, setTitle] = useState(initialTitle);
  const [body, setBody] = useState('');
  const [mode, setMode] = useState<'write' | 'preview'>('write');
  const [banner, setBanner] = useState(false);
  const [until, setUntil] = useState('');
  const act = useActKey();
  const formId = useId();
  const send = useMutation({ mutationFn: () => {
    const request = { title: title.trim(), body, ...(scope.kind === 'system' && banner ? { banner_until: new Date(until).toISOString() } : {}) };
    return sendMessage(org, request, act({ org, request }));
  } });
  const submit = () => { if (unavailableReason === undefined && !send.isPending && send.data === undefined) send.mutate(); };
  const result = send.data;
  const countWords = t(scope.kind === 'org' ? 'adminMessages.sendsToMembers' : 'adminMessages.sendsToPeople', { number: count === undefined ? '–' : formatNumber(count, i18n.language), count: count ?? 0 });
  return <Dialog open title={t(scope.kind === 'org' ? 'adminMessages.messageMembers' : 'adminMessages.messageEveryone')}
    dismissible={!send.isPending} onOpenChange={(open) => !open && !send.isPending && onClose()}
    footer={result !== undefined ? <Button onClick={onClose}>{t('admin.common.done')}</Button> : <>
      <span className="mr-auto min-w-0"><Audience org={org} count={count}>{countWords}</Audience></span>
      <Button disabled={send.isPending} onClick={onClose}>{t('common.cancel')}</Button>
      <Button type="submit" form={formId} variant="primary" pending={send.isPending}
        blocked={unavailableReason !== undefined} reason={unavailableReason}
        disabled={title.trim() === '' || body.trim() === '' || count === undefined || banner && until === ''}>{t('adminMessages.send')}</Button>
    </>}>
    {result !== undefined ? <StateLine role="status" title={t(scope.kind === 'org' ? 'adminMessages.sentToMembers' : 'adminMessages.sentToPeople', { number: formatNumber(result.recipient_count, i18n.language), count: result.recipient_count })} /> :
      <form id={formId} className="flex min-w-0 flex-col gap-4 py-2" onSubmit={(event) => { event.preventDefault(); submit(); }}>
        <Input label={t('adminMessages.title')} value={title} required disabled={send.isPending} autoFocus onChange={(event) => setTitle(event.target.value)} />
        <Tabs label={t('adminMessages.body')} value={mode} onValueChange={setMode} tabs={[
          { value: 'write', label: t('adminMessages.write'), panel: <Textarea label={t('adminMessages.body')} rows={8} value={body} required disabled={send.isPending} onChange={(event) => setBody(event.target.value)} /> },
          { value: 'preview', label: t('adminMessages.preview'), panel: <Markdown text={body} /> },
        ]} />
        {scope.kind === 'system' && <div className="flex flex-wrap items-end gap-3">
          <Checkbox label={t('adminMessages.showBanner')} checked={banner} disabled={send.isPending} onCheckedChange={setBanner} />
          {banner && <DateField label={t('adminMessages.until')} type="datetime-local" value={until} required disabled={send.isPending} onValueChange={setUntil} />}
        </div>}
        {list.isError && <ErrorState compact error={list.error} onRetry={() => void list.refetch()} />}
        {send.error !== null && <ErrorAlert error={send.error} />}
      </form>}
  </Dialog>;
}
