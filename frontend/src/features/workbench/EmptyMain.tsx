/** The same recovery entrance in an empty desktop cell or the phone's selected cell. */
import { useFrameReadiness } from '@/shell/FrameReadiness';
import { SquareDashedMousePointer } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { openFinder } from '@/features/finder/store';
import { ArrangementMenu } from './Arrangements';
import { mainGroups, removeEmptyMainGroup } from './layout';
import { useWorkbench } from './store';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';

export function EmptyMain({ groupId }: { groupId: string | null }) {
  const { t } = useTranslation();
  const frame = useWorkbench();
  const { ready, pending } = useFrameReadiness();
  const group = mainGroups(frame.layout.main).find((entry) => entry.id === groupId);
  if (!ready && pending !== null) return pending;
  return <EmptyState className="h-full" icon={<SquareDashedMousePointer aria-hidden />} title={t('workbench.group.empty')}
    action={<>
      <Button size="sm" onClick={() => { frame.setFocusedGroup(groupId); openFinder(''); }}>{t('workbench.workspace.open')}</Button>
      {group?.retainEmpty === true && <Button size="sm" onClick={() => frame.change((layout) => removeEmptyMainGroup(layout, group.id))}>{t('workbench.workspace.removeGroup')}</Button>}
      <ArrangementMenu screen={frame.screen} current={frame.arrangement} onChoose={frame.choose} onReset={frame.reset} />
      {frame.focusMode && <Button size="sm" onClick={() => frame.setFocusMode(false)}>{t('workbench.workspace.exitFocus')}</Button>}
    </>} />;
}
