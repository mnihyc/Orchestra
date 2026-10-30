/** One contextual view chooser shared by the finder and the title bar. */
import type { TFunction } from 'i18next';
import { PanelsTopLeft, Plus } from 'lucide-react';
import { type MouseEventHandler, useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import { type Command, useRegisterCommands } from '@/features/finder/commands';
import { openFinder } from '@/features/finder/store';
import { Marked } from '@/features/finder/Marked';
import type { FinderRow } from '@/features/finder/rows';
import { containerOf, useContainers, type ContainerDefinition } from '@/features/workbench/containers';
import { findView, type Layout } from '@/features/workbench/layout';
import { viewChoice } from '@/features/workbench/choiceWords';
import { revealView } from '@/features/workbench/openView';
import { useWorkbench } from '@/features/workbench/store';
import { offers, useViews, viewContainer, type ViewDefinition } from '@/features/workbench/views';
import type { ScreenKind } from '@/routes/-access';
import { useCurrentHeading } from '@/shell/screen';
import { IconButton } from '@/ui/IconButton';
import { Kbd } from '@/ui/Kbd';
import { PaletteItem } from '@/ui/Palette';

export function showOpenViews(): void { openFinder('', 'view.open.'); }

export function viewLocation(view: ViewDefinition, layout: Layout, screen: ScreenKind, t: TFunction, containers?: readonly ContainerDefinition[]): string {
  const place = findView(layout, view.name);
  if (place !== null && place.container === undefined) return t(`workbench.areas.${place.area}`);
  const name = place?.container?.name ?? viewContainer(view, screen);
  const container = containers?.find((candidate) => candidate.name === name) ?? containerOf(name);
  return container === undefined ? t(view.title) : t(container.title);
}

export function OpenViewCommands() {
  const registered = useViews();
  const containers = useContainers();
  const screen = useWorkbench((state) => state.screen);
  const layout = useWorkbench((state) => state.layout);
  const heading = useCurrentHeading();
  const commands = useMemo<Command[]>(() => [
    { id: 'view.choose', kind: 'view', title: (t) => t('workbench.openView.title'), keywords: (t) => t('workbench.openView.words').split(/\s+/), icon: PanelsTopLeft, keeps: true, run: showOpenViews },
    ...registered.filter((view) => view.role !== 'content' && offers(view, screen, heading)).map((view): Command => ({
      id: `view.open.${view.name}`, kind: 'view', title: (t) => viewChoice(view, t).name,
      keywords: (t) => [t('workbench.openView.words'), viewLocation(view, layout, screen, t, containers), viewChoice(view, t).description],
      note: (t) => viewLocation(view, layout, screen, t, containers),
      icon: view.icon, run: () => revealView(view.name), available: () => offers(view, screen),
    })),
  ], [registered, containers, screen, layout, heading]);
  useRegisterCommands(commands);
  return null;
}

export function AddViewButton() {
  const { t } = useTranslation();
  return <IconButton size="sm" label={t('workbench.group.add')} icon={<Plus aria-hidden />} onClick={showOpenViews} />;
}

/** View choices use one named row with its container at the right; primary choices need no caption. */
export function WorkbenchChoiceRow({ row, onSelect, onClick }: { row: FinderRow; onSelect: () => void; onClick: MouseEventHandler<HTMLSpanElement> }) {
  const Icon = row.icon;
  return <PaletteItem aria-label={row.title} aria-description={row.disabled ?? row.note} value={row.id} disabled={row.disabled !== undefined} onSelect={onSelect} className="min-w-0">
      <span className="flex min-w-0 flex-1 items-start gap-3" onClick={onClick}>
        <span className="flex h-5 shrink-0 items-center text-text-muted"><Icon aria-hidden /></span><span className="min-w-0 flex-1 leading-5 wrap-break-word"><Marked text={row.title} ranges={row.marks} /></span>
        {row.note !== undefined && <span data-view-group className="min-w-0 max-w-1/2 text-right text-sm leading-5 wrap-break-word text-text-muted">{row.note}</span>}
        {row.shortcut !== undefined && <Kbd shortcut={row.shortcut} />}
      </span>
    </PaletteItem>;
}
