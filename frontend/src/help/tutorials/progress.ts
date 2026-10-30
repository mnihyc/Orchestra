/** Stored progress is a step number, not proof that a screen was used. */
export function boundedStep(step: number, count: number): number {
  return Number.isSafeInteger(step) && count > 0 ? Math.min(Math.max(0, step), count - 1) : 0;
}

/** A visible real control or an authored illustration permits Next; a missing target alone does not. */
export function nextStep(step: number, count: number, ready: boolean): number | 'finished' {
  const current = boundedStep(step, count);
  if (!ready || count < 1) return current;
  return current + 1 === count ? 'finished' : current + 1;
}
