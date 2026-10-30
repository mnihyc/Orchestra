/** The hints of the access screens, by the form each stands in: the three lines of a page for each kind of thing, what sharing a place reaches, the tick that lets people take a thing away as a file, and the choices of sharing with agents.
 *
 * There is one hint in the application (`ui/HelpHint.tsx`, drawn by `features/help/Hint.tsx`), and every hint's id is in one list
 * (`features/help/hints.ts`), which takes these. The words of a hint are in the catalogue under its id
 * (`help.hints.<id>`: the term it stands beside, what the thing is and, where there is a choice, which one to take by what the
 * reader wants). This is the list of them kept as data, so that a test can open each form and name the options that have none.
 */

/** Which forms there are, and the hints in each, in the order they are drawn. */
export const ACCESS_HINTED_FORMS = {
  /** A shared label and the request beside a reference the reader cannot open. */
  'reference-card': ['reference-access'],
  /** The short form of a page: the three lines, for each kind of thing that has them. */
  'page-lines': [
    'line-thread-view',
    'line-thread-interact',
    'line-thread-manage',
    'line-topic-view',
    'line-topic-interact',
    'line-topic-manage',
    'line-project-view',
    'line-project-interact',
    'line-project-manage',
    'line-org-view',
    'line-org-interact',
    'line-org-manage',
    'line-document-view',
    'line-document-interact',
    'line-document-manage',
    'line-environment-view',
    'line-environment-manage',
    'line-resource-view',
    'line-resource-interact',
    'line-resource-manage',
    'line-custom',
  ],
  /** The field that adds people, groups and roles to a page. */
  'share-field': ['share-choice', 'share-reach', 'share-export', 'share-tell', 'share-outside'],
  /** The rows of one act each, and the way the page as a whole stands. */
  'page-rows': ['advanced-rows', 'page-private'],
  /** What goes with a permission, which the short form offers in the same step. */
  'goes-with': ['goes-environment', 'goes-model'],
  /** The tab and the dialog that share with agents. */
  'agents-form': ['agents-whose', 'agents-areas', 'agents-manage', 'agents-copy', 'agents-live', 'agents-download'],
} as const satisfies Record<string, readonly string[]>;

export type AccessHintedForm = keyof typeof ACCESS_HINTED_FORMS;

/** Every hint of the access screens. */
export type AccessHintId = (typeof ACCESS_HINTED_FORMS)[AccessHintedForm][number];

/** The id of the hint that stands beside a line of a page, for a kind of thing: the three lines are worded for each kind apart. */
export function lineHintId(kind: string, line: 'view' | 'interact' | 'manage'): AccessHintId {
  const known = (['thread', 'topic', 'project', 'org', 'document', 'environment'] as const).find((entry) => entry === kind) ?? 'resource';
  // An environment is looked at or managed, so its middle line is the resource's.
  const word = known === 'environment' && line === 'interact' ? 'resource' : known;
  return `line-${word}-${line}` as AccessHintId;
}
