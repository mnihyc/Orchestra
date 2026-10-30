/** The medium-width frame exposes every right container above its sheet, without a picker or hidden labels. */
import { useTranslation } from 'react-i18next';
import { useVisibleLayout } from '@/features/workbench/availability';
import { containerOf, FALLBACK_CONTAINER_ICON } from '@/features/workbench/containers';
import { containerChoice, viewChoice } from '@/features/workbench/choiceWords';
import { selectContainer } from '@/features/workbench/layout';
import { NarrowChoices, type NarrowChoice } from '@/features/workbench/NarrowChoices';
import { useWorkbench } from '@/features/workbench/store';
import { viewOf } from '@/features/workbench/views';

export function OverlayChoices() {
  const { t } = useTranslation();
  const layout = useVisibleLayout();
  const overlay = useWorkbench((state) => state.overlay);
  const change = useWorkbench((state) => state.change);
  const setOverlay = useWorkbench((state) => state.setOverlay);
  const choices = layout.right.containers.map((container): NarrowChoice => {
    const definition = containerOf(container.name);
    const view = viewOf(container.sections[0]?.tab.view ?? '');
    const words = definition === undefined && view !== undefined ? viewChoice(view, t) : containerChoice(container.name, t);
    const Icon = definition?.icon ?? view?.icon ?? FALLBACK_CONTAINER_ICON;
    const active = overlay === 'right' && layout.right.active === container.name;
    return { id: `right:${container.name}`, name: words.name, description: words.description, icon: <Icon aria-hidden />, active,
      onSelect: () => { if (active) setOverlay(null); else { change((current) => selectContainer(current, 'right', container.name)); setOverlay('right'); } } };
  });
  return choices.length === 0 ? null : <NarrowChoices choices={choices} />;
}
