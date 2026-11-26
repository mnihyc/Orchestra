/** Puts the real Chat view into the workbench, in place of the note that said it was not built.
 *
 * A view is registered under its name, and a view registered again takes the place of the one before:
 * this is how the part that builds a view replaces its note. It is imported once, where the shell is
 * put together.
 */
import { MessagesSquare } from 'lucide-react';

import { lazyPart } from '@/features/files/lazy';
import { registerView, type ViewProps } from '@/features/workbench/views';
import { allTabs } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';

registerView({
  name: 'chat',
  title: 'views.chat.title',
  about: 'views.chat.about',
  icon: MessagesSquare,
  screens: ['org', 'project', 'topic', 'thread', 'document'],
  area: 'right',
  singleton: true,
  targeted: () => allTabs(useWorkbench.getState().layout).some((tab) => tab.view === 'chat' && tab.of?.node !== undefined),
  body: lazyPart<ViewProps>(() => import('./ChatPanel').then(({ ChatPanel }) => ({ default: ChatPanel }))),
});
