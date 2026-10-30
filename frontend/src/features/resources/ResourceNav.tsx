/** The row of links between the screens of resources: providers, agent profiles, machines, environments, templates and hosts.
 *
 * It belongs to the section and stands at the head of every list, above the heading of the kind that is in
 * view, since it goes between kinds and not between the parts of one. A resource's own screen has no such
 * row: its way to the list it came from is the path in the title bar, and the arrow beside its name.
 */
import { useViewLocation } from '@/features/workbench/pageContext';
import { Link, useNavigate } from '@tanstack/react-router';
import { Bot, Box, Boxes, Cpu, GraduationCap, Laptop, Plug, PlugZap, Server, type LucideIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import type { ResourceKind } from '@/features/resources/trail';
import { useSettingsSidebarPresence } from '@/features/settings/sidebarPresence';
import { FoldingStrip } from '@/ui/FoldingStrip';

export const KINDS = [
  { to: '/resources/providers', name: 'providers', icon: PlugZap },
  { to: '/resources/agents', name: 'agents', icon: Bot },
  { to: '/resources/machines', name: 'machines', icon: Laptop },
  { to: '/resources/environments', name: 'environments', icon: Boxes },
  { to: '/resources/templates', name: 'templates', icon: Cpu },
  { to: '/resources/hosts', name: 'hosts', icon: Server },
  { to: '/resources/mcp', name: 'mcp', icon: Plug },
  { to: '/resources/mounts', name: 'mounts', icon: Box },
  { to: '/resources/skills', name: 'skills', icon: GraduationCap },
] as const satisfies readonly { to: string; name: ResourceKind; icon: LucideIcon }[];

/** All kind destinations while the native Resources navigation is hidden; never a duplicate of its tree. */
export function ResourceNav() {
  const { t } = useTranslation();
  const pathname = useViewLocation({ select: (location) => location.pathname });
  const navigate = useNavigate();
  const inSidebar = useSettingsSidebarPresence('resources');
  if (inSidebar) return null;
  return (
<div className="flex flex-wrap items-center gap-2" data-resource-navigation>
<div className="min-w-0 flex-1"><FoldingStrip label={t('resources.nav.label')} active={pathname} className="-mt-2 border-b border-border" items={KINDS.map(({ to, name, icon: Icon }) => ({
      id: to, label: t(`screens.${name}.title`), onSelect: () => void navigate({ to }),
      content: <Link to={to} activeProps={{ 'aria-current': 'page' }}
        className="focus-inset relative inline-flex h-(--control-md) min-w-0 max-w-full items-center gap-2 px-3 text-base font-medium whitespace-nowrap text-text-muted hover:text-text aria-[current=page]:text-text">
        <Icon aria-hidden className="shrink-0 text-text-subtle" /><span className="truncate">{t(`screens.${name}.title`)}</span>
      </Link>,
    }))} /></div>
</div>
  );
}
