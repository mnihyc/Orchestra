/** The thread's native permission mode, chosen from its harness and place's offered modes with the current setting revision. */
import { useCompactControls } from '@/features/workbench/Narrow';
import { cx } from '@/ui/cx';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { Check, ChevronDown, Lock } from 'lucide-react';
import { useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceCopy } from '@/i18n/choices';

import { settingsKey } from '@/api/settings';
import { setThreadSetting, threadKeys } from '@/api/threads';
import { useRunsWith } from '@/features/composer/runsWith';
import type { ResolvedSetting, Thread, ThreadParameters } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { Choice } from '@/ui/Choice';
import { ErrorAlert } from '@/ui/ErrorState';
import { Popover } from '@/ui/Popover';
import { DisabledBuildFeatures } from '@/features/resources/agents/ProgramBuild';
import { programNameIn } from '@/features/resources/programNames';

export type NativeApprovalMode = 'ask' | 'on_request' | 'auto' | 'bypass';

export function isApprovalMode(value: string): value is NativeApprovalMode {
  return value === 'ask' || value === 'on_request' || value === 'auto' || value === 'bypass';
}

/** The same picker beside the message and in Parameters; it never invents an offered harness mode. */
export function ApprovalModePicker({ thread, readOnly = false }: { thread: Thread; readOnly?: boolean }) {
  const compact = useCompactControls();
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const descriptionId = useId();
  const [open, setOpen] = useState(false);
  const manages = !readOnly && !thread.archived && thread.side_of === null && holds(thread.access, 'thread.manage');
  const runs = useRunsWith(thread, open && manages);
  const mode = runs.parameters?.approval_mode;
  const words = mode === undefined ? t('composer.mode.checking') : isApprovalMode(mode) ? t(`composer.mode.labels.${mode}`) : mode;
  const options = (runs.parameters?.approval_modes ?? []).filter(isApprovalMode);
  const setting = runs.settings?.find((entry) => entry.key === 'approval.mode');
  const locked = setting?.locked_by;
  const choose = useMutation({
    onMutate: runs.clearRefusal,
    mutationFn: async (next: NativeApprovalMode) => {
      if (!manages || !runs.ready || setting?.writable !== true || locked !== null || !options.includes(next)) return;
      const confirmed = await setThreadSetting(thread.id, 'approval.mode', { value: next, expected_revision: setting.own?.revision ?? 0 });
      const prior = queryClient.getQueryData<ResolvedSetting[]>(settingsKey(thread.id))?.find((entry) => entry.key === confirmed.key);
      // A later live revision can arrive before this response. Keep that newer choice.
      if ((prior?.own?.revision ?? 0) > (confirmed.own?.revision ?? 0)) return;
      queryClient.setQueryData<ResolvedSetting[]>(settingsKey(thread.id), (held) => held?.map((entry) => entry.key === confirmed.key ? confirmed : entry));
      if (typeof confirmed.value === 'string') {
        const confirmedMode = confirmed.value;
        queryClient.setQueryData<ThreadParameters>(threadKeys.parameters(thread.id), (held) => held === undefined ? held : { ...held, approval_mode: confirmedMode });
      }
    },
    onError: (error) => runs.refuse(error),
    onSuccess: () => setOpen(false),
  });
  const writable = manages && runs.ready && setting?.writable === true && locked === null && !choose.isPending;
  const build = thread.binding?.program?.build;
  const reason = readOnly ? t('composer.row.readOnly') : !manages ? t('composer.mode.readOnly', { who: thread.owner.display_name })
    : locked !== undefined && locked !== null ? t('composer.mode.locked', { place: locked.node?.name ?? t('composer.mode.placeAbove') })
      : setting !== undefined && !setting.writable ? t('composer.mode.readOnly', { who: thread.owner.display_name }) : null;
  return <Popover label={t('composer.mode.title')} open={open} onOpenChange={setOpen} side="top" trigger={
    <Button size="sm" variant="ghost" disabled={thread.binding === null} aria-label={t('composer.mode.label', { mode: words })} className={cx('max-w-full', compact ? 'gap-1 px-1 text-xs' : 'gap-2 px-2 text-sm')}>
      <span className="min-w-0">{words}</span><ChevronDown className={cx('size-4 shrink-0', compact && 'hidden')} aria-hidden />
    </Button>
  }>
    <div className="flex max-w-(--container-form) flex-col gap-2" data-approval-mode>
      {build !== undefined && build !== null && <DisabledBuildFeatures build={build} name={programNameIn(thread.binding?.agent, i18n.resolvedLanguage ?? i18n.language) ?? undefined} />}
      {reason !== null && <p className="flex items-center gap-1 text-xs text-text-muted"><Lock aria-hidden />{reason}</p>}
      {options.length === 0 && <p className="text-sm text-text-muted">{t('composer.mode.notOffered')}</p>}
      <ul className="flex flex-col gap-1">
        {options.map((option) => <li key={option}><Choice {...choiceCopy(t, `approvalMode.${option}`)} id={`${descriptionId}-${option}`} compact>{(content, control) => <button {...control} type="button" disabled={!writable} aria-pressed={option === mode}
          className="flex w-full items-start gap-2 rounded-md px-2 py-1 text-left enabled:hover:bg-bg-muted disabled:cursor-default"
          onClick={() => { if (option !== mode) choose.mutate(option); }}>
          <span className="flex size-4 shrink-0 items-center justify-center text-accent-text">{option === mode && <Check aria-hidden />}</span>
          {content}
        </button>}</Choice></li>)}
      </ul>
      <p className="text-xs text-text-muted">{t('composer.mode.applies')}</p>
      {runs.parameters?.approval_reviewer !== undefined && runs.parameters.approval_reviewer !== null && <p className="text-xs text-text-muted">{t('inspector.parameters.reviewer', { model: runs.parameters.approval_reviewer.model })}</p>}
      {runs.refusal !== null && <p role="status" className="text-sm text-warning">{runs.refusal.name === null ? t('composer.mode.stale', { mode: words }) : t('composer.mode.changedBy', { name: runs.refusal.name, mode: words })}</p>}
      {choose.error !== null && runs.refusal === null && <ErrorAlert error={choose.error} />}
      {runs.failed !== null && runs.failed !== undefined && <ErrorAlert error={runs.failed} />}
    </div>
  </Popover>;
}
