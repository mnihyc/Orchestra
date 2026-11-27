import { Link } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import type { Provider } from '@/generated/api-types';
import { ResourceName } from '@/features/resource-facts/ResourceFacts';

/** A connection's location carries the same availability and audience as its environment. */
export function ProviderConnectionEnvironment({provider}:{provider:Provider}) {
  const {t}=useTranslation();
  const environment=provider.connection_environment;
  if(environment===undefined || environment===null)return null;
  return <div className="flex min-w-0 flex-wrap items-center gap-2"><ResourceName subject={{kind:"environment",environment}} asChild hint={false}><Link to="/resources/environments/$id" params={{id:environment.id}}>{environment.name}</Link></ResourceName>
    <span className="text-sm text-text-muted">{t('providerCreation.environmentConnectionState',{state:t(`environments.status.${environment.status}`)})}</span>
  </div>;
}
