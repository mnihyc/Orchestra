/** The system's audit and export.
 * Management uses the same trail as a place, under the system's current session guard.
 */
import { useTranslation } from 'react-i18next';
import { Trail } from '@/features/trace/Trail';
export function Audit() { const { t } = useTranslation(); return <Trail scope={{ type: 'system', name: t('trace.systemTrail') }} embedded />; }
