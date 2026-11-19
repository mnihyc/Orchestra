/** The Inbox view kept in the existing personal state, with a phone-friendly Day/By place select. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { meStateKey, meStateQuery, setMeState } from '@/api/personal';
import { Select } from '@/ui/Select';
import { ErrorAlert } from '@/ui/ErrorState';

export type Grouping = 'day' | 'place';

export function useInboxGrouping() {
  const client = useQueryClient();
  const state = useQuery(meStateQuery);
  const [chosen, setChosen] = useState<Grouping | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [saving, setSaving] = useState(false);
  const inbox = state.data?.inbox;
  const held = typeof inbox === 'object' && inbox !== null ? inbox as Record<string, unknown> : {};
  const value: Grouping = chosen ?? (held.group_by === 'place' ? 'place' : 'day');
  const choose = (next: Grouping) => {
    if (saving) return;
    setChosen(next); setError(null); setSaving(true);
    setMeState('inbox', { ...held, group_by: next }).then(
      () => client.setQueryData<Record<string, unknown>>(meStateKey, (old) => ({ ...old, inbox: { ...held, group_by: next } })),
      (failure: unknown) => setError(failure),
    ).finally(() => setSaving(false));
  };
  return { value, choose, error, saving, loading: state.isPending };
}

export function GroupingControl({ value, onChange, disabled, error }: { value: Grouping; onChange: (value: Grouping) => void; disabled: boolean; error: unknown }) {
  const { t } = useTranslation();
  return <div className="flex flex-col gap-1"><Select label={t('away.inbox.grouping')} value={value} onValueChange={onChange} disabled={disabled} fieldClassName="w-(--field-narrow)" options={[{ value: 'day', ...choiceOption(t, 'inboxGrouping.day') }, { value: 'place', ...choiceOption(t, 'inboxGrouping.place') }]} />{error !== null && <ErrorAlert error={error} message={t('away.inbox.viewSaveFailed')} />}</div>;
}
