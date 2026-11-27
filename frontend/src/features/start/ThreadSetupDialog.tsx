/** Load the binding picker when someone asks to set up their thread. */
import { Suspense, type ComponentProps } from 'react';
import { useTranslation } from 'react-i18next';

import { lazyPart } from '@/features/files/lazy';
import { Dialog } from '@/ui/Dialog';
import { SkeletonRows } from '@/ui/Skeleton';
import type { SetUpThread } from './SetUpThread';

type SetupProps = ComponentProps<typeof SetUpThread>;
const Setup = lazyPart<SetupProps>(() => import('./SetUpThread').then((module) => ({ default: module.SetUpThread })));

export function ThreadSetupDialog(props: SetupProps) {
  const { t } = useTranslation();
  return <Suspense fallback={<Dialog open title={t('threadSetup.title')} onOpenChange={(open) => { if (!open) props.onClose(); }}><SkeletonRows rows={3} /></Dialog>}>
    <Setup {...props} />
  </Suspense>;
}
