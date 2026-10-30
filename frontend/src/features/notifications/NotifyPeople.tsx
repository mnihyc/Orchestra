/** One explicit notification choice, with the actual recipients in the shared list of people. */
import { Hint } from '@/features/help/Hint';
import { useInfiniteQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { moreOperationRecipients } from '@/api/loss';
import type { OperationNotificationPlan } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { Checkbox } from '@/ui/Checkbox';
import { PeopleCard, PeopleList } from '@/ui/PeopleList';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export function NotifyPeople({ plan, value, onChange, disabled = false }: { plan: OperationNotificationPlan | null | undefined; value: boolean | undefined; onChange: (value: boolean) => void; disabled?: boolean }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const [open, setOpen] = useState(false);
  const id = plan?.preview_id ?? '';
  const recipients = useInfiniteQuery({ queryKey: ['operation-recipients', id], initialPageParam: null as string | null, queryFn: ({ pageParam, signal }) => moreOperationRecipients(id, pageParam ?? undefined, signal), getNextPageParam: (page) => page.people.next_cursor, enabled: open && id !== '' });
  if (plan === undefined || plan === null) return null;
  const actual=recipients.data?.pages[0]?.total??plan.people;
  const label = t('operation.notify', { count: actual, number: formatNumber(actual, locale) });
  return <div className="inline-flex min-w-0 items-center gap-2"><Checkbox label={<span className="sr-only">{label}</span>} presentation="compact" checked={value ?? plan.default_on} disabled={disabled || actual === 0} onCheckedChange={onChange} />
    <PeopleCard label={t('ui.avatarStack.list')} onOpenChange={setOpen} trigger={<button type="button" className="min-w-0 truncate text-sm text-text hover:underline" disabled={id === '' || actual === 0}>{label}</button>}>
      {recipients.data === undefined ? recipients.isError ? <ErrorState error={recipients.error} retrying={recipients.isRefetching} onRetry={() => void recipients.refetch()} /> : <SkeletonRows rows={3} /> : <PeopleList users={recipients.data.pages.flatMap((page) => page.people.items)} hidden={recipients.data.pages[0]?.hidden ?? 0}>{recipients.hasNextPage && <Button size="sm" pending={recipients.isFetchingNextPage} onClick={() => void recipients.fetchNextPage()}>{t('operation.more')}</Button>}</PeopleList>}
    </PeopleCard><Hint popover id="loss-notify" /></div>;
}
