/** First administrator. Loader, permission handling and search schema are pending.
 *
 * Open only while the system has no user; the root route sends everybody here until it has one, and
 * away from here afterwards.
 */
import { createFileRoute } from '@tanstack/react-router';

import { Setup } from '@/features/auth/Setup';

export const Route = createFileRoute('/setup/')({
  staticData: { needs: 'setup', frame: 'plain' },
  component: Setup,
});
