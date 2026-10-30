/** Shared with me. Loader, permission handling and search schema are pending.
 *
 * The screen itself belongs to a later part of the work. Its route is here from the start, inside the
 * workbench, so that its address works; until the screen is built it says what it will show.
 */
import { createFileRoute } from '@tanstack/react-router';

import { SharedScreen } from '@/features/shared/Shared';

export const Route = createFileRoute('/shared/')({
  staticData: { screen: 'page' },
  component: Screen,
});

function Screen() {
  return <SharedScreen />;
}
