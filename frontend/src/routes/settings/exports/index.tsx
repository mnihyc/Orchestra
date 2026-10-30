/** Your exports: the archives made for the person, with Download and Remove. The address a finished export's notice links to.
 *
 * A page of the person's own settings, inside the workbench like every other screen. It asks for what it shows
 * once it is drawn, so that its frame is there at once.
 */
import { createFileRoute } from '@tanstack/react-router';

import { YourExports } from '@/features/export/YourExports';

export const Route = createFileRoute('/settings/exports/')({
  staticData: { screen: 'page' },
  component: YourExports,
});
