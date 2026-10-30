/** Inherited or explicit notification level.
 *
 * How much of this place the reader is told of. The bell shows the level that is in force: everything,
 * what matters, or nothing, with its short effective level on the bell itself. The menu sets a level or returns to the
 * actual inherited level. The native acknowledgement supplies both that state and the person's
 * recomputed exceptions page; a refusal leaves the served state and newer thread facts intact.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { defaultLevelName, levelOptions } from '@/features/notifications/levelWords';

import { setNotificationException } from '@/api/notificationExceptions';
import { acceptSubscription, exceptionCursor } from '@/features/notifications/subscription';
import { BellTrigger } from '@/features/notifications/BellTrigger';
import { PauseDialog } from '@/features/settings/personal/away/Pause';
import { OutsideMenu } from '@/features/settings/personal/away/Status';
import type { AwayPrefs } from '@/features/settings/personal/away/types';
import type { SubscriptionLevel, Thread } from '@/generated/api-types';
import { DropdownEntries, Menu, type MenuEntry } from '@/ui/Menu';
import { toast } from '@/ui/Toast';
import { useErrorText } from '@/ui/errorText';

export function SubscriptionBell({ thread, label }: { thread: Pick<Thread, 'id' | 'access'>; label?: string }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const describe = useErrorText();
  const heading = label ?? t('thread.bell.label');
  const own = thread.access.subscription;
  const effective = thread.access.effective_subscription;
  const inherited = thread.access.inherited_subscription ?? null;
  const [open, setOpen] = useState(false);
  const [pause, setPause] = useState<{ mode: 'pause' | 'resume'; prefs: AwayPrefs } | null>(null);
  const change = useMutation({
    mutationKey: ['notification-exception'],
    scope: { id: 'notification-subscriptions' },
    mutationFn: ({ level, cursor }: { level: SubscriptionLevel | null; cursor: string | null }) => setNotificationException(thread.id, level, cursor),
    onMutate: ({ level }) => {
      const key = ['subscription-choice', thread.id] as const;
      const before = cache.getQueryData<SubscriptionLevel | null>(key);
      cache.setQueryData(key, level);
      return { before };
    },
    onError: (error, { level }, context) => {
      const key = ['subscription-choice', thread.id] as const;
      if (context !== undefined && cache.getQueryData(key) === level) {
        if (context.before === undefined) cache.removeQueries({ queryKey: key, exact: true });
        else cache.setQueryData(key, context.before);
      }
      toast({ tone: 'danger', title: t('thread.bell.notSet'), description: describe(error).message });
    },
    onSuccess: (reply, { cursor }) => acceptSubscription(cache, thread.id, reply, cursor),
  });
  const choose = (value: string) => {
    const level = value === 'default' ? null : value as SubscriptionLevel;
    if (change.isPending || level === own) return;
    setOpen(false);
    change.mutate({ level, cursor: exceptionCursor(cache) });
  };
  const entries: MenuEntry[] = [
    { type: 'label', id: 'label', label: heading },
    { type: 'radio', id: 'level', label: heading, value: own ?? 'default',
      onValueChange: choose,
      options: [
        { value: 'default', name: defaultLevelName(t, inherited) },
        ...levelOptions(t, ['all', 'important', 'off'] as const),
      ],
    },
  ];
  return (<>
    <Menu
      label={heading}
      align="end"
      open={open}
      onOpenChange={(next) => { if (!change.isPending) setOpen(next); }}
      content={<><DropdownEntries entries={entries} /><OutsideMenu nodeId={thread.id} onClose={() => setOpen(false)} onPause={(mode, prefs) => { setOpen(false); setPause({ mode, prefs }); }} /></>}
      trigger={<BellTrigger label={heading} level={effective} inherited={own === null} showLevel pending={change.isPending} />}
    />
    {pause !== null && <PauseDialog open mode={pause.mode} initial={pause.prefs} onOpenChange={(on) => { if (!on) setPause(null); }} />}
  </>
  );
}
