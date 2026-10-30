/** The safe live public-opening sign, linking to Ports only for readers of that door. */
import { Link } from '@tanstack/react-router';
import { ExternalLink } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { exposureOf } from './presentation';
import { Tooltip } from '@/ui/Tooltip';

export function PortsExposure({ environment }: { environment: { id: string } }) {
  const { t } = useTranslation();
  const projection = exposureOf(environment);
  if (!projection.exposed) return null;
  const mark = <span className="inline-flex shrink-0 items-center gap-1 text-xs text-warning-text"><ExternalLink size={16} aria-hidden />{t('ports.exposed')}</span>;
  return <Tooltip content={t('ports.exposedReason')}>
    {projection.mayView ?
      <Link to="/resources/environments/$id" params={{ id: environment.id }} search={{ tab: 'ports' }} className="inline-flex">{mark}</Link> : <span>{mark}</span>}
  </Tooltip>;
}
