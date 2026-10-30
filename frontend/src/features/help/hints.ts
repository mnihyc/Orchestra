/** A hint by its id, in up to four parts: what this is, which one for me, where it is set, and more; made from the catalogue and the registry for every row and every setting, and from a descriptor's own text for what an extension declares.
 *
 * A hint is a text of the catalogue under the id its control names (`help.hints.<id>`): its title (the
 * term it stands beside), what the thing is and, where there is a choice, which one to take by what a person
 * wants. The article that treats it is named here, since an address is no text to translate. Where it is set
 * comes from the screen, which knows what it shows. No request is made for any of it. A hint that a screen
 * names and no catalogue holds does not compile: the id is one of the list below, and a test holds both
 * catalogues to a text for each id and the guide to an article and a heading for each link.
 */
import type { TFunction } from 'i18next';

import { ACCESS_HINTED_FORMS, type AccessHintId } from '@/features/access/hints';
import { EXPORT_HINT_IDS, type ExportHintId } from '@/features/export/hints';
import { HINTED_FORMS, resourceHintGuide, type ResourceHintId } from '@/features/resources/hints';
import { i18n, type TranslationKey } from '@/i18n';
import { READING_GUIDE, readingGuideHeading } from '@/help/reading';

/** What each hint leads on to: the article of the guide that treats it, and the heading in it. */
const MORE: Record<string, { article: string; heading: string } | undefined> = {
  'thread-states': { article: 'thread-states', heading: 'read-the-icons' },
  'import-review': { article: 'import-and-export', heading: 'review-an-import' },
  'import-run': { article: 'import-and-export', heading: 'follow-and-undo-an-import' },
  'export-place-format': { article: 'import-and-export', heading: 'export-a-package' },
  'people-mentions': { article: 'people-and-mentions', heading: 'mention-in-a-document' },
  'reference-access': { article: 'references', heading: 'locked-references' },
  'loss-declare': { article: 'recover-lost-work', heading: 'declare-lost' },
  'loss-recover': { article: 'recover-lost-work', heading: 'recover-environments' },
  'loss-switch': { article: 'recover-lost-work', heading: 'choose-a-switch-destination' },
  'loss-notify': { article: 'recover-lost-work', heading: 'tell-the-affected-people' },
  'loss-native': { article: 'recover-lost-work', heading: 'servers-and-mounts' },
  'loss-mounts': { article: 'recover-lost-work', heading: 'servers-and-mounts' },
  'loss-providers': { article: 'recover-lost-work', heading: 'provider-runtimes' },
  'loss-delete-host': { article: 'recover-lost-work', heading: 'remove-a-host' },
  'navigation-arrangements': { article: 'navigation', heading: 'tree-and-recent' },
  'resource-library': { article: 'resource-library', heading: 'choose-a-view' },
  'resource-facts': { article: 'resource-cards', heading: 'inspect-a-choice' },
  references: { article: 'references', heading: 'preview-and-open' },
  'related-items': { article: 'related-items', heading: 'open-related-items' },
  'document-links': { article: 'document-links', heading: 'read-links' },
  'live-work': { article: 'follow-live-work', heading: 'read-progress' },
  'status-items': { article: 'follow-live-work', heading: 'arrange-status-items' },
  'mount-fates': { article: 'check-mount-copies', heading: 'choose-what-follows' },
  skills: { article: 'use-skills', heading: 'enable-published-versions' },
  'plan-mode': { article: 'plan-before-changes', heading: 'plan-a-turn' },
  'plan-answer': { article: 'plan-before-changes', heading: 'decide-the-plan' },
  goals: { article: 'plan-before-changes', heading: 'goals' },
  'switch-history': { article: 'switch-agent-programs', heading: 'choose-history' },
  'held-model': { article: 'message-queue', heading: 'messages-waiting-for-model-or-environment-access' },
  announcements: { article: 'notices-and-announcements', heading: 'send-an-announcement' },
  inbox: { article: 'notices-and-announcements', heading: 'read-your-inbox' },
  'finder-modes': { article: 'finding-things', heading: 'the-finder' },
  'search-results': { article: 'finding-things', heading: 'words-and-similar-meaning' },
  'semantic-search-index': { article: 'semantic-search', heading: 'index-states' },
  'search-scope': { article: 'finding-things', heading: 'filters-and-words-only' },
  'search-file-type': { article: 'finding-things', heading: 'filters-and-words-only' },
  'search-kinds': { article: 'finding-things', heading: 'filters-and-words-only' },
  'search-when': { article: 'finding-things', heading: 'filters-and-words-only' },
  'find-in-conversation': { article: 'finding-things', heading: 'find-in-a-conversation' },
  'setting-lock': { article: 'settings-and-usage', heading: 'locks' },
  'line-org-interact': { article: 'who-can-see-and-do-what', heading: 'who-may-make-projects-topics-and-threads' },
  'line-project-interact': { article: 'who-can-see-and-do-what', heading: 'who-may-make-projects-topics-and-threads' },
  'line-topic-interact': { article: 'who-can-see-and-do-what', heading: 'who-may-make-projects-topics-and-threads' },
};

/** The hints of the finder, of search and of finding in a conversation. */
const FINDING_HINT_IDS = ['finder-modes', 'search-results', 'semantic-search-index', 'search-scope', 'search-file-type', 'search-kinds', 'search-when', 'find-in-conversation', 'navigation-arrangements'] as const;

/** The hint of the settings: the one option whose effect lies in the places below the one being edited. */
const SETTING_HINT_IDS = ['setting-lock'] as const;

const FEATURE_HINT_IDS = ['people-mentions', 'thread-states', 'resource-library', 'resource-facts', 'references', 'related-items', 'document-links', 'live-work', 'status-items', 'mount-fates', 'skills', 'plan-mode', 'plan-answer', 'goals', 'switch-history', 'held-model', 'announcements', 'inbox', 'import-review', 'import-run'] as const;

/** Permanent loss and recovery use the same section identities in either language. */
const LOSS_HINT_IDS = ['loss-declare', 'loss-recover', 'loss-switch', 'loss-notify', 'loss-native', 'loss-mounts', 'loss-providers', 'loss-delete-host'] as const;

export type HintId = (typeof LOSS_HINT_IDS)[number] | (typeof FINDING_HINT_IDS)[number] | (typeof SETTING_HINT_IDS)[number] | (typeof FEATURE_HINT_IDS)[number] | ExportHintId | ResourceHintId | AccessHintId;

/** Every hint there is: those above, those the export dialogs hold, and those the resources and the access screens list by the form each stands in. A part that adds a hint adds its id to a list that ends here and its texts to both catalogues. */
export const HINT_IDS: readonly HintId[] = [
  ...LOSS_HINT_IDS,
  ...FEATURE_HINT_IDS,
  ...FINDING_HINT_IDS,
  ...SETTING_HINT_IDS,
  ...EXPORT_HINT_IDS,
  ...new Set(Object.values(HINTED_FORMS).flat()),
  ...new Set(Object.values(ACCESS_HINTED_FORMS).flat()),
];

export interface HintParts {
  /** The term it stands beside. */
  title: string;
  /** One or two sentences, in the words of the screen. */
  what: string;
  /** The choice by what the person wants, not by its name; absent where there is no choice. */
  which: string | null;
  /** The article that treats it, at the heading, where there is one. */
  article: { id: string; heading: string | null } | null;
}

/** The article a hint leads on to, if it does. */
export function articleOf(id: HintId, language = 'en'): { id: string; heading: string | null } | null {
  if (id === 'finder-modes') return { id: READING_GUIDE.id, heading: readingGuideHeading(language, 'finder') };
  if (id.startsWith('template-')) return resourceHintGuide(id);
  const more = MORE[id];
  if (more === undefined && Object.values(HINTED_FORMS).some((ids) => (ids as readonly string[]).includes(id))) return resourceHintGuide(id);
  return more === undefined ? null : { id: more.article, heading: more.heading };
}

/** What the catalogue says of a hint, in the language in use. */
export function hintOf(id: HintId, t: TFunction, language = 'en'): HintParts {
  const base = `help.hints.${id}` as const;
  const which = `${base}.which`;
  return {
    title: t(`${base}.title`),
    what: t(`${base}.what`),
    // The choice is the one part a hint may not have; a text that is not there is not asked for.
    which: i18n.exists(which) ? t(which as TranslationKey) : null,
    article: articleOf(id, language),
  };
}
