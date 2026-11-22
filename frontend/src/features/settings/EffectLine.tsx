/** When a change of a setting lands and what it reaches, under the setting's row. An environment's own rows and a place's
 * switches of mounts, MCP servers, templates and skills say it alike, through this line: the environment names the threads
 * that use it, a place the environments and threads that still take its value (the setting's reach, read on the row). */
import { useInfiniteQuery } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { settingReachQuery, type SettingReachTarget } from '@/api/settings-reach';
import type { ResolvedSetting, SettingReachCounts } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';

export function EffectLine({ when, reach }: { when: ReactNode; reach?: ReactNode }) {
  const { t } = useTranslation();
  return <span className="text-xs text-text-muted">
    {t('runtimeChanges.takesEffect')}: <span>{when}</span>{reach !== undefined && reach !== null && <> · {reach}</>}
  </span>;
}

type Reaches = 'environments' | 'threads';

/** A place's switches that give the threads under it something, when a change of one lands there, and what it reaches:
 * the environments that take it (and their threads), or the threads themselves. */
function placeSwitch(key: string, t: TFunction): { when: string; reaches: Reaches } | null {
  switch (key) {
    // A connected environment takes its mounts at its next attachment; one Orch runs, at its next reviewed update.
    case 'env.mounts': return { when: t('unitConfiguration.effects.placeMounts'), reaches: 'environments' };
    case 'mcp.enabled': return { when: t('mcpWords.applies.next_turn'), reaches: 'environments' };
    case 'env.template_id': return { when: t('unitConfiguration.effects.update'), reaches: 'environments' };
    case 'agents.skills': return { when: t('mcpWords.applies.next_session'), reaches: 'threads' };
    default: return null;
  }
}

/** The same snapshot as the row's own Where it applies, so that opening it reads nothing again. */
export function reachSnapshot(setting: Pick<ResolvedSetting, 'value' | 'own' | 'locked_by' | 'source' | 'parts'>): string {
  return JSON.stringify([setting.value, setting.own, setting.locked_by, setting.source, setting.parts]);
}

/** Counts as a server answers them: one that does not count environments yet leaves them out. */
export type ReachCounts = Omit<SettingReachCounts, 'environments' | 'environment_threads'> & Partial<Pick<SettingReachCounts, 'environments' | 'environment_threads'>>;

/** What a place's value reaches, in words: its environments with their threads, or its threads. */
export function useReachWords(counts: ReachCounts | undefined, reaches: Reaches): string | undefined {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  if (counts === undefined) return undefined;
  const threads = (count: number) => t('unitConfiguration.effects.threads', { count, number: formatNumber(count, locale) });
  if (reaches === 'threads') return threads(counts.threads);
  if (counts.environments === undefined) return undefined;
  if (counts.environments === 0) return t('unitConfiguration.effects.noEnvironment');
  return t('unitConfiguration.effects.environmentsWithThreads', {
    environments: t('unitConfiguration.effects.environments', { count: counts.environments, number: formatNumber(counts.environments, locale) }),
    threads: threads(counts.environment_threads ?? 0),
  });
}

function PlaceSwitchEffect({ target, setting, when, reaches }: { target: SettingReachTarget; setting: ResolvedSetting; when: string; reaches: Reaches }) {
  const read = useInfiniteQuery(settingReachQuery(target, setting.key, reachSnapshot(setting)));
  const reach = useReachWords(read.data?.pages[0]?.applies, reaches);
  return <EffectLine when={when} reach={reach} />;
}

/** The effect line of a place's switches, for the settings editor of an org, a project or a topic. */
export function usePlaceSwitchEffect(target: SettingReachTarget): (key: string, setting?: ResolvedSetting) => ReactNode {
  const { t } = useTranslation();
  return (key, setting) => {
    const known = placeSwitch(key, t);
    if (known === null || setting === undefined) return undefined;
    return <PlaceSwitchEffect target={target} setting={setting} when={known.when} reaches={known.reaches} />;
  };
}
