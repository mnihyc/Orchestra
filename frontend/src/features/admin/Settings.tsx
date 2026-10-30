/** The system's registry settings.
 *
 * The settings editor of the settings screens, given the system as its place: every key that is set at
 * the system, with where its value comes from and the system's own row, written and taken back through
 * `PUT` and `DELETE /admin/settings/{key}`, and locked for everything below. It is the same editor as an
 * org's, a project's and a topic's, so a key reads and is changed the same way at every scope.
 */
import { useTranslation } from 'react-i18next';

import { SettingsEditor } from '@/features/settings/Editor';

export function Settings() {
  const { t } = useTranslation();
  return <SettingsEditor target={{ kind: 'system' }} scope="system" place={{ name: t('admin.nav.groups.system'), owner: null, access: null, kind: null }} />;
}
