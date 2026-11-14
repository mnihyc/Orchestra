/** An authored schedule freezes one complete draft and saves without starting a thread or turn. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { Trans, useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { meQuery } from '@/api/auth';
import { createInputSchedule, inputScheduleQuery, updateInputSchedule, type InputSchedule } from '@/api/inputSchedules';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import type { ContentPart, CreateThread, Delivery, InputScheduleCreateBody, InputScheduleTarget, InputScheduleUpdateBody, NativeInvocation, Thread } from '@/generated/api-types';
import { Part } from '@/features/timeline/items/UserMessage';
import { editableWords, withEditedWords } from '@/features/composer/queue';
import { NewThreadDialog } from '@/features/start/BindingDialog';
import type { PlaceRef } from '@/features/navigation/places';
import { useAppearance } from '@/lib/appearance';
import { useActKey } from '@/lib/forms';
import { formatDateTime, useNow } from '@/lib/time';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { CodeBlock } from '@/ui/CodeBlock';
import { DateField } from '@/ui/DateField';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Select } from '@/ui/Select';
import { Textarea } from '@/ui/Textarea';
import { TimeZonePicker } from '@/ui/TimeZonePicker';
import { UserChip } from '@/ui/UserChip';
import { fieldsForTiming, timingPreview } from './timing';

const WEEKDAYS = ['agentSchedules.weekdays.1', 'agentSchedules.weekdays.2', 'agentSchedules.weekdays.3', 'agentSchedules.weekdays.4', 'agentSchedules.weekdays.5', 'agentSchedules.weekdays.6', 'agentSchedules.weekdays.7'] as const;
export interface AuthoredScheduleDraft { content: ContentPart[]; nativeAction: NativeInvocation | null; plan: boolean; delivery: Delivery }
export function ScheduleModes({ plan, delivery }: { plan: boolean; delivery: Delivery }) {
  const { t } = useTranslation();
  return <span className="text-xs text-text-muted">{t(plan ? 'sendLater.plan' : 'sendLater.work')} · {t(`sendLater.deliveryModes.${delivery}`)}</span>;
}
export function DraftPreview({ draft, thread }: { draft: Pick<AuthoredScheduleDraft, 'content' | 'nativeAction'>; thread?: string }) {
  const { t } = useTranslation();
  return <div className="flex min-w-0 flex-col gap-3" aria-label={t('sendLater.message')}>
    {draft.content.map((part, index) => <Part key={index} part={part} thread={thread} />)}
    {draft.nativeAction !== null && <details><summary className="cursor-pointer text-sm text-text-muted">{t('sendLater.native')}</summary><CodeBlock code={JSON.stringify(draft.nativeAction.arguments, null, 2)} lang="json" /></details>}
  </div>;
}
interface EditorProps { topic: string; thread?: Thread; draft?: AuthoredScheduleDraft; schedule?: InputSchedule; onClose: () => void; onAccepted?: () => void }
export function ScheduleEditor({ topic, thread, draft, schedule, onClose, onAccepted }: EditorProps) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const me = useQuery(meQuery);
  const locale = useAppearance((state) => state.locale), zone = useAppearance((state) => state.timeZone);
  const now = useNow();
  const [reader] = useState(libraryReader), mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const original = schedule?.definition;
  const [content] = useState(original?.content ?? draft?.content ?? []);
  const [words, setWords] = useState(() => editableWords(content));
  const [nativeAction, setNativeAction] = useState(original?.native_action ?? draft?.nativeAction ?? null);
  const [plan, setPlan] = useState(schedule !== undefined ? schedule.plan : draft?.plan ?? false);
  const [delivery, setDelivery] = useState<Delivery>(schedule !== undefined ? schedule.delivery : draft?.delivery ?? 'queue');
  const [name, setName] = useState(schedule?.name ?? '');
  const [revision, setRevision] = useState(schedule?.revision ?? 0);
  const [current, setCurrent] = useState<InputSchedule | null>(null);
  const [fields, setFields] = useState(() => ({ ...fieldsForTiming(schedule?.timing), zone: schedule?.timing.zone ?? zone }));
  const [targetMode, setTargetMode] = useState(original?.target.kind === 'new_thread' ? 'new_thread' : 'thread');
  const [config, setConfig] = useState<CreateThread>(original?.target.kind === 'new_thread' ? original.target.config : {});
  const [defaults, setDefaults] = useState(() => Object.keys(config).length === 0);
  const [setup, setSetup] = useState(false);
  const key = useActKey();
  const calculated = timingPreview(fields, now);
  const originalFields = fieldsForTiming(schedule?.timing);
  const unchangedTime = schedule !== undefined && JSON.stringify(fields) === JSON.stringify(originalFields);
  const preview = unchangedTime && schedule.timing.kind === 'once' ? { ...calculated, timing: schedule.timing, at: schedule.timing.at, error: Date.parse(schedule.timing.at) > now ? null : 'future' as const } : calculated;
  const author = schedule?.author ?? me.data;
  const threadId = original?.target.kind === 'thread' ? original.target.thread_id : thread?.id;
  const parts = schedule === undefined ? content : withEditedWords(content, words);
  const allowed = author !== undefined && me.data?.id === author.id && (schedule === undefined || original !== null) && preview.timing !== null
    && (targetMode === 'new_thread' || threadId !== undefined) && (schedule === undefined || name.trim() !== '') && (words.trim() !== '' || parts.some((part) => !('type' in part) || part.type !== 'text') || nativeAction !== null);
  const save = useMutation({ mutationFn: async () => {
    if (!allowed || preview.timing === null || !sameLibraryReader(reader)) throw new DOMException('The schedule is not ready for this reader.', 'AbortError');
    const { first_input: _first, ...choices } = config;
    const target: InputScheduleTarget = targetMode === 'new_thread' ? { kind: 'new_thread', config: defaults ? {} : choices } : { kind: 'thread', thread_id: threadId ?? '' };
    const definition: InputScheduleCreateBody = { name: name.trim() === '' ? null : name.trim(), target, content: parts, native_action: nativeAction, plan, delivery, timing: preview.timing };
    if (schedule === undefined) return createInputSchedule(topic, definition, key(definition));
    const body: InputScheduleUpdateBody = { ...definition, revision, state: null };
    return updateInputSchedule(schedule.id, body, key(body));
  }, onSuccess: (accepted) => {
    if (!mounted.current || !sameLibraryReader(reader)) return;
    if (!('projection' in accepted)) cache.setQueryData(inputScheduleQuery(accepted.id).queryKey, accepted);
    onAccepted?.(); onClose();
  }, onError: (error) => {
    if (schedule !== undefined && error instanceof ApiError && error.is('conflict.stale_revision') && sameLibraryReader(reader)) {
      void cache.query({ ...inputScheduleQuery(schedule.id), staleTime: 0 }).then((row) => { if (mounted.current && sameLibraryReader(reader)) setCurrent(row); }).catch(() => undefined);
    }
  } });
  const parent: PlaceRef = { id: topic, kind: 'topic', name: thread?.path.find((step) => step.id === topic)?.name ?? schedule?.topic?.name ?? '', parentId: null, archived: false, status: null, folder: false };
  const pending = save.isPending;
  const next = preview.at === null ? preview.skipped === null ? '–' : t('sendLater.skipped', { time: preview.skipped, zone: fields.zone }) : formatDateTime(preview.at, { locale, timeZone: fields.zone });
  const updateFields = (patch: Partial<typeof fields>) => { setFields((before) => ({ ...before, ...patch })); save.reset(); };
  return <>
    <Dialog open title={t(schedule === undefined ? 'sendLater.title' : 'sendLater.editTitle')} dismissible={!pending} onOpenChange={(open) => { if (!open && !pending) onClose(); }}
      footer={<><span className="me-auto min-w-0 text-sm text-text-muted">{preview.error === null ? undefined : t(`sendLater.errors.${preview.error}`)}</span><Button variant="ghost" disabled={pending} onClick={onClose}>{t('common.cancel')}</Button><Button type="button" variant="primary" pending={pending} disabled={!allowed || current !== null} onClick={() => save.mutate()}>{t('common.save')}</Button></>}>
      <div className="flex min-w-0 flex-col gap-4">
        {schedule === undefined ? <DraftPreview draft={{ content, nativeAction }} thread={thread?.id} /> : <>
          <Textarea label={t('sendLater.message')} value={words} onChange={(event) => setWords(event.target.value)} disabled={pending} rows={4} />
          <DraftPreview draft={{ content: parts.filter((part) => !('type' in part) || part.type !== 'text'), nativeAction }} thread={threadId} />
          {nativeAction !== null && <Button size="sm" variant="ghost" disabled={pending} onClick={() => setNativeAction(null)}>{t('sendLater.removeNative')}</Button>}
        </>}
        <div className="flex flex-wrap items-center gap-3">
          <Checkbox label={t('sendLater.plan')} checked={plan} onCheckedChange={setPlan} disabled={pending} />
          <Select label={t('sendLater.delivery')} value={delivery} onValueChange={setDelivery} disabled={pending}
            options={(['queue', 'steer', 'interrupt'] as const).map((choice) => ({ value: choice, name: t(`sendLater.deliveryModes.${choice}`) }))} />
        </div>
        <Input label={t('sendLater.name')} optional={schedule === undefined} value={name} onChange={(event) => setName(event.target.value)} disabled={pending} />
        <SegmentedControl label={t('sendLater.when')} value={fields.mode} onValueChange={(mode) => updateFields({ mode })}
          segments={[{ value: 'once', label: t('sendLater.once'), disabled: pending }, { value: 'repeat', label: t('sendLater.repeat'), disabled: pending }]} />
        {fields.mode === 'once' ? <DateField label={t('sendLater.date')} value={fields.date} onValueChange={(date) => updateFields({ date, fold: '' })} disabled={pending} required />
          : <Select label={t('sendLater.frequency')} value={fields.frequency} onValueChange={(frequency) => updateFields({ frequency })} disabled={pending} options={[{ value: 'daily', name: t('sendLater.daily') }, { value: 'weekly', name: t('sendLater.weekly') }]} />}
        <Input type="time" label={t('sendLater.time')} value={fields.time} onChange={(event) => updateFields({ time: event.target.value, fold: '' })} disabled={pending} required />
        <TimeZonePicker label={t('sendLater.zone')} value={fields.zone} mode="fixed" onValueChange={(value) => updateFields({ zone: value.timeZone, fold: '' })} disabled={pending} />
        {fields.mode === 'repeat' && fields.frequency === 'weekly' && <fieldset className="flex flex-wrap gap-3"><legend className="mb-2 text-sm font-medium">{t('sendLater.weekdays')}</legend>{WEEKDAYS.map((word, index) => <Checkbox key={word} label={t(word)} checked={fields.weekdays.includes(index + 1)} onCheckedChange={(checked) => updateFields({ weekdays: checked ? [...fields.weekdays, index + 1] : fields.weekdays.filter((day) => day !== index + 1) })} disabled={pending} />)}</fieldset>}
        {fields.mode === 'once' && preview.ambiguous && <Select label={t('sendLater.fold')} value={fields.fold} onValueChange={(fold) => { if (fold === 'first' || fold === 'second') updateFields({ fold }); }} disabled={pending} options={[{ value: '', name: t('sendLater.choose') }, { value: 'first', name: t('sendLater.first') }, { value: 'second', name: t('sendLater.second') }]} />}
        <div className="flex flex-wrap gap-2 text-sm text-text-muted"><span>{t('sendLater.next')}</span><span>{next}</span><span>{fields.zone}</span></div>
        <Select label={t('sendLater.destination')} value={targetMode} onValueChange={setTargetMode} disabled={pending} options={[{ value: 'thread', name: t('sendLater.thisThread'), disabled: threadId === undefined }, { value: 'new_thread', name: t('sendLater.newThread') }]} />
        {targetMode === 'new_thread' && <div className="flex flex-wrap items-center gap-2"><Checkbox label={t('sendLater.topicDefaults')} checked={defaults} onCheckedChange={setDefaults} disabled={pending} /><Button size="sm" disabled={pending || defaults} onClick={() => setSetup(true)}>{t('sendLater.configure')}</Button></div>}
        {author !== undefined && <p className="flex flex-wrap items-baseline gap-1 text-sm text-text-muted"><Trans i18nKey="sendLater.authority" components={{ person: <UserChip user={author} /> }} /></p>}
        {save.isError && <ErrorAlert error={save.error} />}
        {current !== null && <div className="flex min-w-0 flex-col gap-3"><details><summary className="cursor-pointer text-sm">{t('sendLater.current')}</summary><DraftPreview draft={{ content: current.definition?.content ?? [], nativeAction: current.definition?.native_action ?? null }} thread={current.destination.thread?.id} /><p className="text-sm text-text-muted">{current.name}</p><ScheduleModes plan={current.plan} delivery={current.delivery} /></details><Button size="sm" onClick={() => { setRevision(current.revision); setCurrent(null); save.reset(); key.reset(); }}>{t('sendLater.useRevision')}</Button></div>}
      </div>
    </Dialog>
    {setup && <NewThreadDialog parent={parent} open onClose={() => setSetup(false)} prepare={{ initial: config, onChoose: setConfig }} />}
  </>;
}
