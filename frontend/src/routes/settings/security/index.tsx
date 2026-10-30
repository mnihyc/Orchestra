/** Personal security. Loader, permission handling and search schema are pending.
 *
 * The page is the person's own, inside the workbench like every other screen. Everything it asks for
 * it asks for itself once it is drawn, so that the page shows its frame at once.
 */
import { createFileRoute } from '@tanstack/react-router';

import { Security } from '@/features/settings/personal/Security';

export const Route = createFileRoute('/settings/security/')({
  staticData: { screen: 'page' },
  component: Security,
});
