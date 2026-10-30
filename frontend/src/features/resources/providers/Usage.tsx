/** The usage screen opened on a provider: every thread that ran on it, of whatever org.
 * The same ledger observation and applicable limits as its opening card.
 */
import type { Provider } from '@/generated/api-types';
import { Limits } from '@/features/usage/Limits';
import { Report } from '@/features/usage/Report';
import { holds } from '@/lib/permissions';
import type { UsageSearch } from '@/features/usage/search';
import type { SearchPatch } from '@/features/usage/choices';

export function ProviderUsage({ provider,search,onSearch }: { provider: Provider;search?:UsageSearch | undefined;onSearch?:((patch:SearchPatch)=>void) | undefined }) {
  return <Report key={provider.id} scope={{kind:'provider',id:provider.id}} providerData={provider} search={search} onSearch={onSearch} embedded limitsSlot={<Limits of={{kind:'provider',id:provider.id,name:provider.name,manage:holds(provider.access,'provider.manage')}} />} />;
}
