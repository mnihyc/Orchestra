/** Credential-free observations of a completed native copy; unknown values never imply readiness. */
const notes = ['credentials_not_copied', 'sign_in_not_copied', 'source_consent_reset'] as const;
export type CopySetupNote = typeof notes[number];
export function copySetupNotes(value: unknown): CopySetupNote[] | undefined {
  if (!Array.isArray(value) || value.some((note: unknown) => !notes.some((known) => note === known))) return undefined;
  return notes.filter((note) => value.includes(note));
}
