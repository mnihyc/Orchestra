/** The native approval and sandbox choices share their reader, offered bounds and revision-guarded writer. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { Check, ChevronDown, Lock } from 'lucide-react';
import { useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { client } from '@/api/client';
import { settingsKey } from '@/api/settings';
import { setThreadSetting, threadKeys } from '@/api/threads';
import { reportedMode, type ModeKind } from '@/features/composer/nativeModes';
import { useRunsWith } from '@/features/composer/runsWith';
import { DisabledBuildFeatures } from '@/features/resources/agents/ProgramBuild';
import { programNameIn } from '@/features/resources/programNames';
import { nativeChangeValue } from '@/features/timeline/items/Marker';
import { useCompactControls } from '@/features/workbench/Narrow';
import type { ResolvedSetting, Thread, ThreadParameters } from '@/generated/api-types';
import { offeredChoice } from '@/i18n/choices';
import { holds } from '@/lib/permissions';
import { useThreadPart } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { Choice } from '@/ui/Choice';
import { cx } from '@/ui/cx';
import { ErrorAlert } from '@/ui/ErrorState';
import { Popover } from '@/ui/Popover';
import { Tooltip } from '@/ui/Tooltip';

export type NativeApprovalMode = 'ask' | 'on_request' | 'auto' | 'bypass';
export function isApprovalMode(value: string): value is NativeApprovalMode {
  return value === 'ask' || value === 'on_request' || value === 'auto' || value === 'bypass';
}
/** The same approval control in the composer and the Parameters entry point. */
export function ApprovalModePicker(props: { thread: Thread; readOnly?: boolean }) {
  return <NativeModePicker {...props} kind="approval" />;
}
export function ConfinementPicker(props: { thread: Thread; readOnly?: boolean }) {
  return <NativeModePicker {...props} kind="sandbox" />;
}

function NativeModePicker({ thread, readOnly = false, kind }: { thread: Thread; readOnly?: boolean; kind: ModeKind }) {
  const compact = useCompactControls();
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const descriptionId = useId();
  const [open, setOpen] = useState(false);
  const manages = !readOnly && !thread.archived && thread.side_of === null && holds(thread.access, 'thread.manage');
  // Thread opening and the composer already supply these exact cache entries. No new reader.
  const runs = useRunsWith(thread, true);
  const settingKey = kind === 'approval' ? 'approval.mode' : 'sandbox.mode';
  const setting = runs.settings?.find((entry) => entry.key === settingKey);
  const raw = kind === 'approval' ? runs.parameters?.approval_mode : setting?.value;
  const mode = typeof raw === 'string' ? raw : undefined;
  const title = t(kind === 'approval' ? 'composer.mode.title' : 'composer.confinement.title');
  const copy = (value: string) => offeredChoice(t, kind === 'approval' ? 'approvalMode' : 'sandboxLevel', value)
    ?? { name: value, description: t('composer.mode.nativeChoice') };
  const words = mode === undefined ? t('composer.mode.checking') : copy(mode).name;
  const options = kind === 'approval' ? (runs.parameters?.approval_modes ?? setting?.available_values ?? []) : (setting?.available_values ?? []);
  const locked = setting?.locked_by;
  const inherited = setting !== undefined && setting.source.scope !== 'thread';
  const report = useThreadPart(thread.id, (view) => reportedMode(view, thread.current_segment_id, kind));
  const divergence = report?.marker === 'harness_changed' && report.data.diverged ? report.data : null;
  const choose = useMutation({
    onMutate: runs.clearRefusal,
    mutationFn: async (next: string) => {
      if (!manages || !runs.ready || setting?.writable !== true || (setting.not_applicable_reason !== null && setting.not_applicable_reason !== undefined) || locked !== null || !options.includes(next)) return;
      const author = client.session.getState();
      const confirmed = await setThreadSetting(thread.id, settingKey, { value: next, expected_revision: setting.own?.revision ?? 0 });
      const now = client.session.getState();
      if (now.userId !== author.userId || now.readerVersion !== author.readerVersion || now.status !== author.status) return;
      const prior = queryClient.getQueryData<ResolvedSetting[]>(settingsKey(thread.id))?.find((entry) => entry.key === confirmed.key);
      if ((prior?.own?.revision ?? 0) > (confirmed.own?.revision ?? 0)) return;
      queryClient.setQueryData<ResolvedSetting[]>(settingsKey(thread.id), (held) => held?.map((entry) => entry.key === confirmed.key
        ? { ...confirmed, available_values: confirmed.available_values ?? entry.available_values } : entry));
      if (kind === 'approval' && typeof confirmed.value === 'string') {
        const confirmedMode = confirmed.value;
        queryClient.setQueryData<ThreadParameters>(threadKeys.parameters(thread.id), (held) => held === undefined ? held : { ...held, approval_mode: confirmedMode });
      }
    },
    onError: (error) => runs.refuse(error),
    onSuccess: () => setOpen(false),
  });
  const writable = manages && runs.ready && setting?.writable === true && (setting.not_applicable_reason === null || setting.not_applicable_reason === undefined) && locked === null && !choose.isPending;
  const build = thread.binding?.program?.build;
  const reason = readOnly ? t('composer.row.readOnly') : !manages ? t(kind === 'approval' ? 'composer.mode.readOnly' : 'composer.confinement.readOnly', { who: thread.owner.display_name })
    : setting?.not_applicable_reason ?? (locked !== undefined && locked !== null ? t('composer.mode.locked', { place: locked.node?.name ?? t('composer.mode.placeAbove') })
      : setting !== undefined && !setting.writable ? t(kind === 'approval' ? 'composer.mode.readOnly' : 'composer.confinement.readOnly', { who: thread.owner.display_name }) : null);
  const blocked = reason ?? (choose.isPending ? t('composer.mode.saving') : (runs.failed !== null && runs.failed !== undefined) ? t('composer.mode.unavailable') : t('composer.mode.checking'));
  const unbound = thread.binding === null;
  const label = t(kind === 'approval' ? 'composer.mode.label' : 'composer.confinement.label', { mode: words });
  return <span className="inline-flex min-w-0 max-w-full items-center gap-1" data-native-mode={kind}>
    <Popover label={title} open={open} onOpenChange={setOpen} side="top" trigger={
      <Button size="sm" variant="ghost" disabled={unbound} reason={unbound ? t('composer.row.unbound') : reason ?? undefined} aria-label={label}
        className={cx('min-w-0 max-w-full', compact ? 'gap-1 px-1 text-xs' : 'gap-2 px-2 text-sm')}>
        <span className="min-w-0 truncate">{words}</span>{inherited && <span className="text-xs text-text-subtle" data-mode-default>{t(kind === 'approval' ? 'composer.mode.placeDefault' : 'composer.confinement.placeDefault')}</span>}
        <ChevronDown className={cx('size-4 shrink-0', compact && 'hidden')} aria-hidden />
      </Button>
    }>
      <div className="flex max-w-(--container-form) flex-col gap-2" data-approval-mode={kind}>
        {kind === 'approval' && build !== undefined && build !== null && <DisabledBuildFeatures build={build} name={programNameIn(thread.binding?.agent, i18n.resolvedLanguage ?? i18n.language) ?? undefined} />}
        {reason !== null && <p className="flex items-center gap-1 text-xs text-text-muted"><Lock aria-hidden />{reason}</p>}
        {options.length === 0 && <p className="text-sm text-text-muted">{t(kind === 'approval' ? 'composer.mode.notOffered' : 'composer.confinement.notOffered')}</p>}
        <ul className="flex flex-col gap-1">
          {options.map((option) => <li key={option}><Choice {...copy(option)} id={`${descriptionId}-${option}`} compact>{(content, control) =>
            <Tooltip content={writable ? copy(option).description : blocked}><button {...control} type="button" disabled={!writable} aria-description={writable ? undefined : blocked} aria-pressed={option === mode}
              className="flex w-full items-start gap-2 rounded-md px-2 py-1 text-left enabled:hover:bg-bg-muted disabled:cursor-default"
              onClick={() => { if (option !== mode) choose.mutate(option); }}>
              <span className="flex size-4 shrink-0 items-center justify-center text-accent-text">{option === mode && <Check aria-hidden />}</span>{content}
            </button></Tooltip>}</Choice></li>)}
        </ul>
        <p className="text-xs text-text-muted">{t('composer.mode.applies')}</p>
        {kind === 'approval' && runs.parameters?.approval_reviewer !== null && runs.parameters?.approval_reviewer !== undefined && <p className="text-xs text-text-muted">{t('inspector.parameters.reviewer', { model: runs.parameters.approval_reviewer.model })}</p>}
        {runs.refusal !== null && <p role="status" className="text-sm text-warning">{runs.refusal.name === null ? t('composer.mode.stale', { mode: words }) : t('composer.mode.changedBy', { name: runs.refusal.name, mode: words })}</p>}
        {choose.error !== null && runs.refusal === null && <ErrorAlert error={choose.error} />}
        {(runs.failed !== null && runs.failed !== undefined) && <ErrorAlert error={runs.failed} action={<Button size="sm" onClick={() => { void Promise.all([queryClient.refetchQueries({ queryKey: settingsKey(thread.id), exact: true }), queryClient.refetchQueries({ queryKey: threadKeys.parameters(thread.id), exact: true })]); }}>{t('common.tryAgain')}</Button>} />}
      </div>
    </Popover>
    {divergence !== null && <Tooltip content={t('composer.mode.lastReported', { program: divergence.program, mode: nativeChangeValue(divergence.change, divergence.value, t), time: divergence.reported_at })}>
      <span tabIndex={0} className="max-w-(--chip-width-wide) truncate text-xs text-warning" data-native-mode-report>{t('composer.mode.reported', { program: divergence.program, mode: nativeChangeValue(divergence.change, divergence.value, t) })}</span>
    </Tooltip>}
  </span>;
}
