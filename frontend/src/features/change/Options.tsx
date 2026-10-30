/** The parameters of a plan with the values they start from: a running turn, the wait, the checks, keeping the old side.
 *
 * Few, with sensible defaults, and nothing requires opening them: what to do with a turn that is running (wait for it
 * up to some minutes, or stop it now), how long the new side has to pass its checks, whether the old side is kept, and
 * whether this thread's processes in the environment it leaves are ended. A value that is not what it starts from is
 * marked, with Reset. A fork has two more that are its own: whose page the new thread has, and its name.
 */
import { RotateCcw } from 'lucide-react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { type Choices, type OptionChoices } from '@/features/change/choices';
import type { AppliedChangeOptions, ForkPlan } from '@/generated/api-types';
import { IconButton } from '@/ui/IconButton';
import { Checkbox } from '@/ui/Checkbox';
import { useFocusFirst } from '@/features/change/Sentence';
import { Input } from '@/ui/Input';
import { RadioGroup } from '@/ui/RadioGroup';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { ChoiceFieldHelp } from '@/ui/ChoiceField';

/** The values the options start from while the plan does not say them yet: the settings as shipped. */
const SHIPPED: AppliedChangeOptions = { active_turn: 'wait', keep_previous: false, stop_processes: true, verify_timeout_secs: 120, wait_minutes: 10 };

function Row({ title, resetLabel, changed, onReset, children, help }: { title: string; resetLabel: string; changed: boolean; onReset: () => void; children: ReactNode; help?: ReactNode }) {
  return (
    <div className="flex flex-col gap-2 border-t border-border pt-3 first:border-t-0 first:pt-0" data-changed={changed ? 'true' : undefined}>
      <div className="flex items-start gap-2">
        <div className="flex min-w-0 flex-1 items-center gap-1"><h4 className={changed ? 'min-w-0 text-sm leading-(--control-sm) font-medium text-text' : 'min-w-0 text-sm font-medium text-text'}>{title}</h4>{help}</div>
        {changed && (
          <IconButton size="sm" label={resetLabel} icon={<RotateCcw aria-hidden />} onClick={onReset} />
        )}
      </div>
      {children}
    </div>
  );
}

export interface OptionsProps {
  choices: Choices;
  onChange: (patch: Partial<Choices>) => void;
  /** What the plan took them from; the settings as shipped until a plan has come. */
  applied: AppliedChangeOptions | null;
  /** The plan, for what is asked only where it matters: the old side, other threads' processes, notes. */
  plan: ForkPlan | null;
  /** The topic whose access defaults a fork may follow. */
  topic: string;
}

export function Options({ choices, onChange, applied, plan, topic }: OptionsProps) {
  const { t } = useTranslation();
  const start = applied ?? SHIPPED;
  const set = (patch: Partial<OptionChoices>) => onChange({ options: { ...choices.options, ...patch } });
  const o = choices.options;
  const fork = choices.result === 'new';
  const activeTurn = o.activeTurn ?? start.active_turn;
  const leavesEnvironment = (plan?.leaves?.environment ?? null) !== null;
  const notes = plan?.memory ?? null;
  const othersStay = plan?.leaves?.environment?.action === 'in_use';
  const ref = useFocusFirst<HTMLDivElement>();
  const oldOptions = [
    { value: 'remove' as const, name: t('changeCompact.old.remove'), description: t('choiceCatalog.oldRuntime.remove.description') },
    { value: 'keep' as const, name: t('changeCompact.old.keep'), description: t('choiceCatalog.oldRuntime.keep.description') },
  ];
  const accessOptions = [
    { value: 'original' as const, name: t('changeCompact.access.original'), description: t('fork.page.originalAbout') },
    { value: 'place' as const, name: t('changeCompact.access.place'), description: t('changeCompact.accessFromTopic', { topic }) },
    { value: 'private' as const, name: t('changeCompact.access.private'), description: t('fork.page.privateAbout') },
  ];

  return (
    <div ref={ref} className="flex flex-col gap-3 rounded-lg border border-border bg-bg-subtle p-4" data-change-options>
      {!fork && (
        <Row title={t('change.options.turn')} resetLabel={t('changeCompact.reset.turn')} changed={o.activeTurn !== null || o.waitMinutes !== null} onReset={() => set({ activeTurn: null, waitMinutes: null })}>
          <div className="flex flex-wrap items-end gap-3">
            <SegmentedControl
              label={t('change.options.turn')}
              value={activeTurn}
              onValueChange={(next) => set({ activeTurn: next })}
              segments={[
                { value: 'wait', name: t('changeCompact.turn.wait') },
                { value: 'interrupt', name: t('changeCompact.turn.interrupt') },
              ]}
            />
            {activeTurn === 'wait' && (
              <Input
                label={t('change.options.minutes')}
                type="number"
                min={0}
                max={1440}
                inputMode="numeric"
                value={String(o.waitMinutes ?? start.wait_minutes)}
                onChange={(event) => set({ waitMinutes: Math.min(1_440, Math.max(0, Number(event.target.value) || 0)) })}
                fieldClassName="w-(--field-narrow)"
              />
            )}
          </div>
        </Row>
      )}
      <Row title={t('change.options.checks')} resetLabel={t('changeCompact.reset.checks')} changed={o.verifySecs !== null} onReset={() => set({ verifySecs: null })}>
        <Input
          label={t('change.options.checksSeconds')}
          type="number"
          min={10}
          max={3600}
          inputMode="numeric"
          value={String(o.verifySecs ?? start.verify_timeout_secs)}
          onChange={(event) => set({ verifySecs: Math.min(3_600, Math.max(10, Number(event.target.value) || 10)) })}
          fieldClassName="w-(--field-narrow)"
        />
      </Row>
      {!fork && leavesEnvironment && (
        <Row
          title={t('change.options.oldSide')}
          help={<ChoiceFieldHelp label={t('change.options.oldSide')} options={oldOptions} />}
          resetLabel={t('changeCompact.reset.previous')}
          changed={o.keepPrevious !== null}
          onReset={() => set({ keepPrevious: null })}
        >
            <RadioGroup
              variant="list"
              label={t('change.options.oldSide')}
              value={(o.keepPrevious ?? start.keep_previous) ? 'keep' : 'remove'}
              onValueChange={(next) => set({ keepPrevious: next === 'keep' })}
              options={oldOptions}
            />
        </Row>
      )}
      {!fork && othersStay && (
        <Row title={t('change.options.processes')} resetLabel={t('changeCompact.reset.processes')} changed={o.stopProcesses !== null} onReset={() => set({ stopProcesses: null })}>
          <RadioGroup
            variant="list"
            label={t('change.options.processes')}
            value={(o.stopProcesses ?? start.stop_processes) ? 'end' : 'leave'}
            onValueChange={(next) => set({ stopProcesses: next === 'end' })}
            options={[
              { value: 'end', name: t('changeCompact.processes.end') },
              { value: 'leave', name: t('changeCompact.processes.leave') },
            ]}
          />
        </Row>
      )}
      {fork && (
        <Row
          title={t('fork.page.title')}
          help={<ChoiceFieldHelp label={t('fork.page.title')} options={accessOptions} />}
          resetLabel={t('changeCompact.reset.access')}
          changed={choices.page !== 'original'}
          onReset={() => onChange({ page: 'original' })}
        >
            <RadioGroup
              variant="list"
              label={t('fork.page.title')}
              value={choices.page}
              onValueChange={(page) => onChange({ page })}
              options={accessOptions}
            />
        </Row>
      )}
      {notes !== null && (
        <Row title={t('fork.notes.title')} resetLabel={t('changeCompact.reset.notes')} changed={choices.memory !== null} onReset={() => onChange({ memory: null })}>
          <Checkbox label={t('fork.notes.take', { count: notes.files })} checked={choices.memory ?? notes.carried} onCheckedChange={(memory) => onChange({ memory })} />
        </Row>
      )}
      {fork && (
        <Row title={t('fork.name.title')} resetLabel={t('changeCompact.reset.name')} changed={choices.name.trim() !== ''} onReset={() => onChange({ name: '' })}>
          <Input label={t('fork.name.label')} hideLabel placeholder={t('fork.name.placeholder')} value={choices.name} onChange={(event) => onChange({ name: event.target.value })} autoComplete="off" />
        </Row>
      )}
    </div>
  );
}
