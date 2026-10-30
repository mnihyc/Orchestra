/** Your imports: jobs and retained reports for the signed-in person. */
import { createFileRoute } from '@tanstack/react-router';

import { YourImports } from '@/features/people/YourImports';

export const Route = createFileRoute('/settings/imports/')({
  staticData: { screen: 'page' },
  component: YourImports,
});
