/** Recover access. Loader, permission handling and search schema are pending.
 */
import { createFileRoute } from '@tanstack/react-router';

import { Recovery } from '@/features/auth/Recovery';

export const Route = createFileRoute('/recover/')({
  staticData: { needs: 'signed_out', frame: 'plain' },
  component: Recovery,
});
