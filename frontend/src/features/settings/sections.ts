/** Settings destinations shared by the screens, the palette and the route-coverage gate. */
export const ORG_SECTIONS = ['general', 'departments', 'members', 'groups', 'roles', 'policies', 'tags', 'service-accounts', 'audit', 'everything', 'channels', 'destinations'] as const;
export const PROJECT_SECTIONS = ['general', 'policies', 'tags', 'audit', 'destinations'] as const;
export const TOPIC_SECTIONS = ['general', 'policies', 'audit', 'destinations'] as const;

export type ProjectSection = (typeof PROJECT_SECTIONS)[number];

export function isProjectSection(value: unknown): value is ProjectSection {
  return typeof value === 'string' && (PROJECT_SECTIONS as readonly string[]).includes(value);
}

/** The sections of a topic's settings, in the order a project's navigation shows them (those that apply to a topic): the tab opens on the first. */
export type TopicSection = (typeof TOPIC_SECTIONS)[number];

export function isTopicSection(value: unknown): value is TopicSection {
  return typeof value === 'string' && (TOPIC_SECTIONS as readonly string[]).includes(value);
}

/** destinations remains an addressed alias; the org's actual navigation offers Outside only once. */
export function orgSections(flat = false): readonly (typeof ORG_SECTIONS)[number][] {
  return ORG_SECTIONS.filter((section) => section !== 'destinations' && (section !== 'departments' || !flat));
}
