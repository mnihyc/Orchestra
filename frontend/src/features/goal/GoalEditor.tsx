/** The goal editor: the objective and "Done when", limits behind a disclosure, and whether to be told when it ends.
 *
 * Two fields are what a goal needs. The limits default to none and follow the place's settings; a limit left empty
 * follows its setting, and a place that locked a setting shows its value and holds the field. One dialog serves a new
 * goal and the Edit of one under way.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { setGoal, updateGoal } from '@/api/goals';
import { settingsQuery } from '@/api/settings';
import { closeGoalEditor, useGoalEditorThread } from '@/features/goal/editor';
import { useGoalPlans } from '@/features/goal/plan';
import { Hint } from '@/features/help/Hint';
import type { GoalLimits, Thread, ThreadGoal } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { formatSpan } from '@/lib/time';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { Disclosure } from '@/ui/Disclosure';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Textarea } from '@/ui/Textarea';
import { isLive } from '@/features/goal/model';

/** The most characters of each of the two fields: what the server holds. */
const OBJECTIVE_MAX = 4_000;
const CONDITION_MAX = 2_000;

const LIMIT_KEYS = { turns: 'thread.goal.max_turns', minutes: 'thread.goal.max_minutes', credits: 'thread.goal.max_credits' } as const;
type LimitField = keyof typeof LIMIT_KEYS;

/** What a field of limits holds as the server takes it: nothing follows the setting, a number is the goal's own. */
function limitsOf(fields: Record<LimitField, string>): GoalLimits {
  const whole = (text: string) => (text.trim() === '' || !/^\d+$/.test(text.trim()) ? null : Number(text.trim()));
  const credits = fields.credits.trim();
  return { max_turns: whole(fields.turns), max_minutes: whole(fields.minutes), max_credits: /^\d+(\.\d+)?$/.test(credits) ? credits : null };
}

function editorFields(goal: ThreadGoal | null | undefined): { objective: string; condition: string; limits: Record<LimitField, string>; notify: boolean } {
  const live = goal !== null && goal !== undefined && (isLive(goal) || goal.status === 'stopped') ? goal : null;
  return {
    objective: live?.objective ?? '',
    condition: live?.condition ?? '',
    limits: {
      turns: live?.limits.max_turns?.toString() ?? '',
      minutes: live?.limits.max_minutes?.toString() ?? '',
      credits: live?.limits.max_credits ?? '',
    },
    notify: live?.notify_on_end ?? false,
  };
}

function Form({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const read = useThreadReadRef(thread.id);
  const existing = thread.goal !== null && thread.goal !== undefined && (isLive(thread.goal) || thread.goal.status === 'stopped') ? thread.goal : null;
  const initial = editorFields(thread.goal);
  const [objective, setObjective] = useState(initial.objective);
  const [condition, setCondition] = useState(initial.condition);
  const [limits, setLimits] = useState(initial.limits);
  const [notify, setNotify] = useState(initial.notify);
  const plans = useGoalPlans(thread);
  const settings = useQuery({ ...settingsQuery(thread.id) });
  const placeValue = (field: LimitField): { shown: string; locked: boolean } => {
    const setting = settings.data?.find((one) => one.key === LIMIT_KEYS[field]);
    const raw = setting?.value;
    const zero = raw === undefined || raw === null || raw === 0 || raw === '0' || raw === '';
    const locked = setting?.locked_by !== undefined && setting.locked_by !== null;
    return { shown: zero ? t('goals.editor.noLimit') : typeof raw === 'number' ? formatNumber(raw, locale) : typeof raw === 'string' ? raw : t('goals.editor.noLimit'), locked };
  };
  const save = useMutation({
    mutationFn: () => existing === null
      ? setGoal(read, { objective: objective.trim(), condition: condition.trim(), limits: limitsOf(limits), notify_on_end: notify, plan: plans })
      : updateGoal(read, {
        ...(objective.trim() === existing.objective ? {} : { objective: objective.trim() }),
        ...(condition.trim() === existing.condition ? {} : { condition: condition.trim() }),
        limits: limitsOf(limits),
        notify_on_end: notify,
      }),
    onSuccess: closeGoalEditor,
  });
  const ready = objective.trim() !== '' && condition.trim() !== '';
  const own = limitsOf(limits);
  const summary = [
    own.max_turns === null ? placeValue('turns').shown : own.max_turns === 0 ? '' : t('goals.line.turns', { count: own.max_turns }),
    own.max_minutes === null ? placeValue('minutes').shown : own.max_minutes === 0 ? '' : formatSpan(own.max_minutes * 60_000, { locale }, 'short'),
    own.max_credits === null ? placeValue('credits').shown : Number(own.max_credits) === 0 ? '' : t('goals.line.credits', { credits: own.max_credits }),
  ].filter((part) => part !== '' && part !== t('goals.editor.noLimit'));
  const field = (name: LimitField, label: string) => {
    const place = placeValue(name);
    return (
      <Input key={name} label={label} type="number" inputMode="numeric" min={0} value={limits[name]} placeholder={place.shown}
        disabled={place.locked} {...(place.locked ? { hint: t('goals.editor.lockedBy') } : {})}
        onChange={(event) => setLimits((held) => ({ ...held, [name]: event.target.value }))} />
    );
  };
  return (
    <Dialog open onOpenChange={(open) => { if (!open && !save.isPending) closeGoalEditor(); }} size="md"
      title={<span className="inline-flex items-center gap-1">{t(existing === null ? 'goals.editor.titleSet' : 'goals.editor.titleEdit')}<Hint id="goals" popover /></span>}
      dismissible={!save.isPending}
      footer={<>
        <span className="me-auto min-w-0 text-sm text-text-muted" role="status">
          {!ready ? t('goals.editor.needBoth') : plans && existing === null ? t('goals.editor.planFirst') : ''}
        </span>
        <Button disabled={save.isPending} onClick={closeGoalEditor}>{t('common.cancel')}</Button>
        <Button variant="primary" pending={save.isPending} disabled={!ready || save.isPending} onClick={() => save.mutate()}>
          {t(existing === null ? 'goals.editor.set' : 'goals.editor.save')}
        </Button>
      </>}>
      <form className="flex min-w-0 flex-col gap-3" onSubmit={(event) => { event.preventDefault(); if (ready && !save.isPending) save.mutate(); }}>
        <Textarea label={t('goals.editor.objective')} rows={3} maxLength={OBJECTIVE_MAX} value={objective} onChange={(event) => setObjective(event.target.value)} data-autofocus />
        <Textarea label={t('goals.editor.condition')} rows={2} maxLength={CONDITION_MAX} value={condition} onChange={(event) => setCondition(event.target.value)} />
        <Disclosure title={summary.length === 0 ? t('goals.editor.limitsNone') : t('goals.editor.limitsSome', { summary: summary.join(' · ') })}>
          <div className="grid gap-3 py-3 sm:grid-cols-3">
            {field('turns', t('goals.editor.turns'))}
            {field('minutes', t('goals.editor.minutes'))}
            {field('credits', t('goals.editor.credits'))}
          </div>
        </Disclosure>
        <Checkbox label={t('goals.editor.notify')} checked={notify} onCheckedChange={setNotify} />
        {save.error !== null && <ErrorAlert error={save.error} />}
      </form>
    </Dialog>
  );
}

/** Mounted once by the thread's screen: the editor, while it is open for this thread. */
export function GoalEditorHost({ thread }: { thread: Thread }) {
  const open = useGoalEditorThread();
  return open === thread.id ? <Form key={thread.goal?.id ?? 'new'} thread={thread} /> : null;
}
