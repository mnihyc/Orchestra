/** What a thread runs with, read and changed: the parameters of its agent with the thread's own choices among them, kept current as the thread's history says they changed, and the writes of a choice.
 *
 * The parameters are asked for once for a thread and shared by every control that shows them (the box and
 * the header both do). The central refresh registry asks again when an act or a live thread row says they
 * changed, including before any history exists; the thread's own choices are asked for only where a control
 * is opened, since they decide who a choice is credited to and what a change of model leaves behind.
 */
import { type QueryClient, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useViewShown } from '@/features/workbench/viewOpening';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { modelDisplayName } from '@/features/models/names';
import { useShallow } from 'zustand/react/shallow';

import { changeThreadChoices } from '@/api/models';
import { ApiError } from '@/api/client';
import { settingsQuery } from '@/api/settings';
import { parametersQuery, setThreadSetting } from '@/api/threads';
import { clearThreadSetting } from '@/api/workspace';
import { type Chooser, chooserOf, droppedFor, type State, statesOf, withChoice, withoutDropped } from '@/features/composer/modelState';
import type { Item, ModelOption, ResolvedSetting, Thread, ThreadParameters } from '@/generated/api-types';
import { useThreadPart } from '@/realtime/stores/thread';
import { refreshThread } from '@/realtime/live';

/** How many of the newest items are looked through for a line that says a setting changed. */
const NEWEST_LOOKED_AT = 40;

type SettingLine = Extract<Item, { kind: 'marker' }>;

const isSettingLine = (item: Item | undefined): item is SettingLine =>
  item?.kind === 'marker' && (item.payload.marker === 'setting_changed' || item.payload.marker === 'model_changed');

/** The newest lines of the thread's history that say a setting or the model changed, oldest first. */
function newestSettingLines(items: ReadonlyMap<string, Item>, order: readonly string[]): SettingLine[] {
  const found: SettingLine[] = [];
  for (let at = order.length - 1; at >= 0 && order.length - at <= NEWEST_LOOKED_AT; at -= 1) {
    const item = items.get(order[at] ?? '');
    if (isSettingLine(item)) found.unshift(item);
  }
  return found;
}

/** The thread's own choices, from the settings in effect at the thread; none while they are not read. */
export function ownChoices(settings: readonly ResolvedSetting[] | undefined): Record<string, unknown> | null {
  if (settings === undefined) return null;
  const value = settings.find((setting) => setting.key === 'agent.params')?.own?.value;
  return typeof value === 'object' && value !== null && !Array.isArray(value) ? { ...(value as Record<string, unknown>) } : {};
}

/** Every model chooser keeps compatible thread choices and reports each incompatible choice it removes. */
export async function changeModelChoice(thread: Pick<Thread, 'id' | 'model'>, choice: { model: string; effort: string | null; option: ModelOption }, states: readonly State[], settings: readonly ResolvedSetting[]) {
  await setThreadSetting(thread.id, 'agent.model', { value: choice.model, expected_revision: settingOf(settings, 'agent.model').own?.revision ?? 0 });
  if (choice.effort !== null) await setThreadSetting(thread.id, 'agent.effort', { value: choice.effort, expected_revision: settingOf(settings, 'agent.effort').own?.revision ?? 0 });
  if (choice.model === thread.model) return { dropped: [], option: choice.option };
  const own = ownChoices(settings) ?? {};
  const dropped = droppedFor(states, own, choice.option.parameters);
  if (dropped.length > 0) await changeThreadChoices(thread.id, (choices) => withoutDropped(choices, dropped), settingOf(settings, 'agent.params'));
  return { dropped, option: choice.option };
}

function settingOf(settings: readonly ResolvedSetting[] | undefined, key: string): ResolvedSetting {
  const setting = settings?.find((one) => one.key === key);
  if (setting === undefined) throw new Error('The current choice has not been read yet.');
  return setting;
}

export interface SettingRefusal {
  key: string;
  member?: string;
  name: string | null;
  model: string;
}

export const isSettingConflict = (error: unknown): error is ApiError => error instanceof ApiError && error.is('conflict.stale_revision');

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** A refused write reads the current values and exact reported changer before another choice is possible. */
async function readRefusal(threadId: string, error: unknown, queryClient: QueryClient): Promise<SettingRefusal | null> {
  if (!isSettingConflict(error)) return null;
  const current = error.problem.current;
  const by = error.problem.changed_by;
  refreshThread(threadId);
  const [settings, parameters] = await Promise.all([
    queryClient.query({ ...settingsQuery(threadId), staleTime: 0 }),
    queryClient.query({ ...parametersQuery(threadId), staleTime: 0 }),
  ]);
  const model = parameters.model.options.find((option) => option.model_name === parameters.model.value);
  const key = isRecord(current) && typeof current.key === 'string' ? current.key : 'agent.model';
  const reportedOwn = isRecord(current) && isRecord(current.own) ? current.own : null;
  const own = settings.find((setting) => setting.key === key)?.own;
  // A later switch can overtake the refusal's reads. Credit only the exact reported revision,
  // and, for a model switch, the same routing name the parameters now display.
  const sameRevision = reportedOwn !== null && own !== undefined && own !== null && reportedOwn.revision === own.revision;
  const sameModel = key !== 'agent.model' || (isRecord(current) && current.value === parameters.model.value);
  return {
    key,
    name: sameRevision && sameModel && isRecord(by) && typeof by.display_name === 'string' ? by.display_name : null,
    model: parameters.model.value === null ? '' : modelDisplayName(model?.display_name),
  };
}

export interface RunsWith {
  parameters: ThreadParameters | undefined;
  settings: readonly ResolvedSetting[] | undefined;
  ready: boolean;
  refusal: SettingRefusal | null;
  refuse: (error: unknown, member?: string) => Promise<void>;
  clearRefusal: () => void;
  write: (key: string, value: unknown) => Promise<void>;
  reset: (key: string) => Promise<void>;
  /** The thread's own choices, where they are read. */
  own: Record<string, unknown> | null;
  states: State[];
  pending: boolean;
  failed: unknown;
  /** Who made the choice of a parameter, as far as the history that is held says. */
  chooser: (key: string) => Chooser | null;
  /** Gives a parameter a value, or none to follow what the model starts with. */
  choose: (state: State, value: string | number | null) => void;
  choosing: boolean;
  chooseError: unknown;
}

/** What a thread runs with. `withOwn` reads the thread's own choices as well, for a control that is opened. */
export function useRunsWith(thread: Thread, withOwn: boolean): RunsWith {
  const { t } = useTranslation();
  const shown = useViewShown();
  const queryClient = useQueryClient();
  // A thread that is not bound to anything yet runs with nothing to choose.
  const bound = thread.binding !== null;
  const parameters = useQuery({ ...parametersQuery(thread.id), enabled: bound && shown });
  const settings = useQuery({ ...settingsQuery(thread.id), enabled: withOwn && bound && shown });
  const [refusal, setRefusal] = useState<SettingRefusal | null>(null);
  const written = useThreadPart(
    thread.id,
    useShallow((view) => newestSettingLines(view.items, view.order)),
  );
  // Live thread rows and successful acts share the central refresh registry, including no-turn
  // settings changes. History stays here only to credit a displayed choice to its maker.
  const refuse = async (error: unknown, member?: string): Promise<void> => {
    const next = await readRefusal(thread.id, error, queryClient);
    if (next !== null) setRefusal({ ...next, ...(member === undefined ? {} : { member }) });
  };

  const write = async (key: string, value: unknown): Promise<void> => {
    setRefusal(null);
    const direct = settings.data?.find((one) => one.key === key);
    if (direct !== undefined) {
      await setThreadSetting(thread.id, key, { value, expected_revision: direct.own?.revision ?? 0 });
    } else {
      const member = key.startsWith('agent.params.') ? key.slice('agent.params.'.length) : key;
      await changeThreadChoices(thread.id, (choices) => ({ ...choices, [member]: value }), settingOf(settings.data, 'agent.params'));
    }
  };
  const reset = async (key: string): Promise<void> => {
    setRefusal(null);
    const direct = settings.data?.find((one) => one.key === key);
    if (direct !== undefined) {
      if (direct.own !== null) await clearThreadSetting(thread.id, key, direct.own.revision);
    } else {
      const member = key.startsWith('agent.params.') ? key.slice('agent.params.'.length) : key;
      await changeThreadChoices(thread.id, (choices) => Object.fromEntries(Object.entries(choices).filter(([name]) => name !== member)), settingOf(settings.data, 'agent.params'));
    }
  };

  const own = withOwn ? ownChoices(settings.data) : null;
  const states = parameters.data === undefined ? [] : statesOf(parameters.data, own, t);

  const choose = useMutation({
    mutationFn: (change: { state: State; value: string | number | null }) => changeThreadChoices(thread.id, (choices) => withChoice(choices, change.state, change.value), settingOf(settings.data, 'agent.params')),
    onMutate: () => setRefusal(null),
    onError: (error, change) => refuse(error, change.state.key),
  });

  return {
    parameters: parameters.data,
    settings: settings.data,
    ready: settings.data !== undefined && settings.error === null && !settings.isFetching && !parameters.isFetching,
    refusal,
    refuse,
    clearRefusal: () => setRefusal(null),
    write,
    reset,
    own,
    states,
    pending: bound && (parameters.isPending || (withOwn && settings.isPending)),
    failed: parameters.error ?? settings.error,
    chooser: (key) =>
      chooserOf(
        written.flatMap((line) => (line.payload.marker === 'setting_changed' ? [{ key: line.payload.data.key, from: line.payload.data.from, to: line.payload.data.to, by: line.payload.data.by }] : [])),
        key,
      ),
    choose: (state, value) => choose.mutate({ state, value }),
    choosing: choose.isPending,
    chooseError: choose.error,
  };
}
