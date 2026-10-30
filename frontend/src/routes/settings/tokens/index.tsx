/** Personal tokens. Loader, permission handling and search schema are pending.
 *
 * The page is the person's own, inside the workbench like every other screen. Everything it asks for
 * it asks for itself once it is drawn, so that the page shows its frame at once.
 */
import { createFileRoute } from '@tanstack/react-router';

import { Tokens } from '@/features/settings/personal/Tokens';

export const Route = createFileRoute('/settings/tokens/')({
  staticData: { screen: 'page' },
  component: Tokens,
});
