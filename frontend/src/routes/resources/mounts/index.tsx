/** Mounts listed with readable records and private permission-page names.
 *
 * A private row carries its name and owner and opens its permission page. Readable mounts open their own pages.
 */
import { createFileRoute } from '@tanstack/react-router';

import { ResourceNames } from '@/features/resources/ResourceNames';

export const Route = createFileRoute('/resources/mounts/')({
  staticData: { screen: 'page' },
  component: Screen,
});

function Screen() {
  return <ResourceNames kind="mounts" />;
}
