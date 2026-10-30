/** Skills listed with readable records and private permission-page names.
 *
 * A private row carries its name and owner and opens its permission page. Readable skills open their own pages.
 */
import { createFileRoute } from '@tanstack/react-router';

import { SkillList } from '@/features/resources/skills/SkillList';

export const Route = createFileRoute('/resources/skills/')({
  staticData: { screen: 'page' },
  component: Screen,
});

function Screen() {
  return <SkillList />;
}
