/** The hints of the export dialogs, by id: what ticking a place takes, the two forms, how much of a conversation, what is left out, how long an archive is kept, and the span of days.
 *
 * Their words are in the catalogues under `help.hints.<id>`, where every hint's are; the list is taken in by
 * the one list of hints, which holds both catalogues to a text for each.
 */

/** Every hint the export dialogs draw. */
export const EXPORT_HINT_IDS = ['export-what', 'export-format', 'export-place-format', 'export-detail', 'export-left-out', 'export-kept', 'export-span'] as const;

export type ExportHintId = (typeof EXPORT_HINT_IDS)[number];
