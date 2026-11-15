/** How long nothing may arrive from a running agent, with no tool running, before the status line says how long nothing came.
 *
 * The thread's `thread.silent_after_secs` decides, from the place it is set at; until the page has read the thread's settings, and where
 * it may not read them, the registry's own default (two minutes) stands.
 */
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import type { ResolvedSetting } from '@/generated/api-types';

const KEY = 'thread.silent_after_secs';
const registered = SETTINGS_REGISTRY.find((def) => def.key === KEY)?.default;
/** The shipped default, in milliseconds, read from the registry so that it is stated once. */
export const DEFAULT_SILENT_AFTER_MS = (typeof registered === 'number' ? registered : 120) * 1_000;

/** The silence that is told, in milliseconds, from the thread's settings as the page holds them (or none yet). */
export function silentAfterMs(settings: readonly ResolvedSetting[] | undefined): number {
  const value = settings?.find((setting) => setting.key === KEY)?.value;
  return typeof value === 'number' && Number.isFinite(value) && value >= 1 ? value * 1_000 : DEFAULT_SILENT_AFTER_MS;
}
