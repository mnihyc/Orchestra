/** A model alone is saved through the thread's ordinary revision-guarded setting writer. */
import { useMutation } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import type { Choices } from '@/features/change/choices';
import { Frame } from '@/features/change/Frame';
import { RunsOn } from '@/features/change/RunsOn';
import { Where } from '@/features/change/Where';
import { leftBehind } from '@/features/composer/ModelEffort';
import { optionName } from '@/features/composer/modelRows';
import { changeModelChoice, useRunsWith } from '@/features/composer/runsWith';
import { SettingConflict } from '@/features/composer/SettingConflict';
import type { Thread } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { toast } from '@/ui/Toast';

export function ModelSettingChange({ thread, choices, onChange, simple, open, onClose }: {
  thread: Thread; choices: Choices; onChange: (patch: Partial<Choices>) => void; simple: boolean; open: boolean; onClose: () => void;
}) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const runs = useRunsWith(thread, true);
  const option = runs.parameters?.model.options.find((model) => model.model_name === choices.model);
  const writable = holds(thread.access, 'thread.manage') && runs.parameters?.model.writable === true
    && (choices.effort === undefined || choices.effort === null || choices.effort === runs.parameters.effort.value || runs.parameters.effort.writable);
  const save = useMutation({
    mutationFn: () => {
      if (option === undefined) throw new Error('The chosen model has not been read');
      const effort = choices.effort === runs.parameters?.effort.value ? null : choices.effort ?? null;
      return changeModelChoice(thread, { model: option.model_name, effort, option }, runs.states, runs.settings ?? []);
    },
    onMutate: runs.clearRefusal,
    onError: async (error) => { await runs.refuse(error); },
    onSuccess: ({ dropped, option }) => {
      toast({ tone: 'success', title: t('composer.model.slash.done', { what: t('composer.model.chip'), value: optionName(option) }),
        description: dropped.length === 0 ? t('composer.model.applies.next_turn') : dropped.map((entry) => leftBehind(entry, option, locale, t)).join(' ') });
      onClose();
    },
  });
  const canSave = runs.ready && option?.allowed === true && writable && !save.isPending;
  const reason = save.isPending ? t('changeReady.applying') : runs.failed !== null && runs.failed !== undefined ? t('changeReady.failed')
    : !runs.ready || option === undefined ? t('changeReady.checking') : !writable || !option.allowed ? t('providerOptions.unavailable') : t('composer.model.applies.next_turn');
  return <Frame title={t('change.title.switch', { name: thread.name })} steps={false} size={simple ? 'md' : 'lg'} at="plan" open={open} onClose={onClose} dismissible={!save.isPending}
    footer={<><span role="status" className="min-w-0 flex-1 text-sm text-text-muted">{reason}</span>
      <Button disabled={save.isPending} onClick={onClose}>{t('common.cancel')}</Button>
      <Button variant="primary" blocked={!canSave} pending={save.isPending} onClick={() => { if (canSave) save.mutate(); }}>{t('common.save')}</Button></>}>
    <fieldset disabled={save.isPending} className="flex min-w-0 flex-col gap-3">
      <RunsOn thread={thread} choices={choices} onChange={onChange} compact={simple} />
      {!simple && <Where thread={thread} choices={choices} onChange={onChange} fork={false} />}
    </fieldset>
    {runs.failed !== null && runs.failed !== undefined && <ErrorAlert error={runs.failed} action={<Button size="sm" onClick={() => {
      for (const read of runs.reads ?? []) if (read.isError === true) void read.refetch?.();
    }}>{t('common.tryAgain')}</Button>} />}
    {runs.refusal !== null && <SettingConflict refusal={runs.refusal} />}
    {save.error !== null && runs.refusal === null && <ErrorAlert error={save.error} />}
  </Frame>;
}
