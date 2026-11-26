/** How long nothing may arrive from a running agent, with no tool running, before the status line says how long nothing came.
 *
 * The native first-screen facts carry the effective setting, including the registry default. Until that fact is known,
 * the activity stays factual and no guessed silence notice appears.
 */
/** Seconds already resolved by the native reader; an absent fact cannot establish a silence. */
export function silentAfterMs(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 1 ? value * 1_000 : Number.POSITIVE_INFINITY;
}
