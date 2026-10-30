/** Help sections share real, bookmarkable destinations between the page and its sidebar. */
import { Link } from '@tanstack/react-router';
import type { TFunction } from 'i18next';
import { BookOpen, GraduationCap, Keyboard, Newspaper, SlidersHorizontal } from 'lucide-react';
import type { SectionNavItem } from '@/ui/SectionNav';
import { client } from '@/api/client';
import { isPlainClick } from '@/features/chat/open';
import { setHelpQuery } from '@/features/help/state';
import { writeViewChoice } from '@/lib/viewChoices';

export const HELP_SECTIONS = [
  { value: 'articles', title: 'help.parts.articles', Icon: BookOpen },
  { value: 'build', title: 'helpProvider.buildYourOwn', Icon: BookOpen },
  { value: 'settings', title: 'helpRelease.settings', Icon: SlidersHorizontal },
  { value: 'tutorials', title: 'help.parts.tutorials', Icon: GraduationCap },
  { value: 'keys', title: 'help.parts.keys', Icon: Keyboard },
  { value: 'news', title: 'helpRelease.news.title', Icon: Newspaper },
] as const;
export type HelpSection = (typeof HELP_SECTIONS)[number]['value'];

export function helpSection(value: unknown): HelpSection | undefined {
  return HELP_SECTIONS.find((item) => item.value === value)?.value;
}
export function helpSearch(search: Record<string, unknown>): { part?: HelpSection } {
  const part = helpSection(search.part);
  return part === undefined ? {} : { part };
}
export function helpSections(t: TFunction): SectionNavItem[] {
  return HELP_SECTIONS.map(({ value, title, Icon }) => ({ id: value, label: t(title), icon: <Icon aria-hidden />,
    link: <Link to="/help" search={{ part: value }} onClick={(event) => {
      if (!isPlainClick(event)) return;
      setHelpQuery('');
      writeViewChoice(client.session.getState().userId, 'help', 'part', value);
    }} /> }));
}
