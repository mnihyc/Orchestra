/** A workbench door to the same native background review, independent of the environment's processes. */
import { useStore } from 'zustand';
import { useTranslation } from 'react-i18next';
import { client } from '@/api/client';
import { ThreadPanel } from '@/features/inspector/ThreadPanel';
import { useViewShown } from '@/features/workbench/viewOpening';
import type { ViewProps } from '@/features/workbench/views';
import type { Thread } from '@/generated/api-types';
import { threadReadOnly, useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { RunningWorkPanel } from './Running';

function CurrentWork({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(thread.id);
  const reader = useStore(client.session, (state) => state.readerVersion);
  const readOnly = useThreadPart(read, threadReadOnly);
  return readOnly ? <p role="status" className="p-3 text-sm text-text-muted">{t('composer.row.readOnly')}</p>
    : <RunningWorkPanel key={JSON.stringify([read.key, reader])} thread={thread.id} />;
}
export function BackgroundWorkView({ tab }: ViewProps) {
  const shown = useViewShown();
  return !shown ? null : <ThreadPanel threadId={tab.of?.thread}>{(thread) => <CurrentWork thread={thread} />}</ThreadPanel>;
}
