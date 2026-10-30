/** The working sections of Management: their destinations, icons and navigation groups. */
import {
  Bot,
  Boxes,
  Building,
  Cable,
  Coins,
  Database,
  FileKey,
  Gauge,
  KeyRound,
  LayoutTemplate,
  ListTree,
  type LucideIcon,
  Mail,
  Puzzle,
  ScanSearch,
  ScrollText,
  Server,
  Settings,
  ShieldQuestionMark,
  SlidersHorizontal,
  Users,
  Wrench,
} from 'lucide-react';

import type { AdminSearch } from '@/features/admin/search';

export type SectionGroup = 'people' | 'machines' | 'agents' | 'rules' | 'system';

export interface SectionDef {
  /** The last segment of the address. */
  id: string;
  /** Where it stands in the list at the side; the first section stands alone. */
  group: SectionGroup | null;
  icon: LucideIcon;
}

/** In the order they are listed. */
export const SECTIONS = [
  { id: 'overview', group: null, icon: Gauge },
  { id: 'users', group: 'people', icon: Users },
  { id: 'orgs', group: 'people', icon: Building },
  { id: 'everything', group: 'people', icon: ListTree },
  { id: 'sign-in', group: 'people', icon: KeyRound },
  { id: 'email', group: 'people', icon: Mail },
  { id: 'hosts', group: 'machines', icon: Server },
  { id: 'runtimes', group: 'machines', icon: Boxes },
  { id: 'templates', group: 'machines', icon: LayoutTemplate },
  { id: 'mcp', group: 'machines', icon: Wrench },
  { id: 'capabilities', group: 'agents', icon: ShieldQuestionMark },
  { id: 'harnesses', group: 'agents', icon: Cable },
  { id: 'agents', group: 'agents', icon: Bot },
  { id: 'plugins', group: 'agents', icon: Puzzle },
  { id: 'defaults', group: 'rules', icon: SlidersHorizontal },
  { id: 'rates', group: 'rules', icon: Coins },
  { id: 'settings', group: 'rules', icon: Settings },
  { id: 'storage', group: 'system', icon: Database },
  { id: 'semantic-search', group: 'system', icon: ScanSearch },
  { id: 'system', group: 'system', icon: Server },
  { id: 'pki', group: 'system', icon: FileKey },
  { id: 'audit', group: 'system', icon: ScrollText },
] as const satisfies readonly SectionDef[];

export type SectionId = (typeof SECTIONS)[number]['id'];

/** The groups in the order they are listed. */
export const GROUPS: readonly SectionGroup[] = ['people', 'machines', 'agents', 'rules', 'system'];

/** Whether the section is one of Management's destinations. */
export function isListed(section: SectionDef): boolean {
  return SECTIONS.some((entry) => entry.id === section.id);
}

/** The sections that are listed, in their order. */
export const LISTED: readonly (typeof SECTIONS)[number][] = SECTIONS.filter((section) => isListed(section));

/** Whether an address segment names a Management section. */
export function isSection(value: string): value is SectionId {
  return LISTED.some((section) => section.id === value);
}

export function sectionOf(id: SectionId): SectionDef {
  const found = SECTIONS.find((section) => section.id === id);
  // `id` is one of the ids by its type.
  return found ?? SECTIONS[0];
}

/** What every section is given: the search of its address, and the way to change it. */
export interface SectionProps {
  search: AdminSearch;
  onSearch: (patch: { [Member in keyof AdminSearch]?: AdminSearch[Member] | undefined }, options?: { replace?: boolean }) => void;
}
