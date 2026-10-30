/** Provider names use the shared place label, with only the origins the reader may see. */
import type { Provider, ProviderRef } from '@/generated/api-types';
import { foldName, type KnownPlaces } from '@/lib/placeLabel';
import { resourceLabels } from '@/features/resources/labels';

export type ProviderNameData = Pick<Provider, 'id' | 'name' | 'home' | 'owner' | 'org_id'>;

export function providerLabels(providers: readonly ProviderNameData[], known: KnownPlaces): ReadonlyMap<string, string> {
  return resourceLabels(providers, known);
}

/** Whether readable origins are needed to distinguish different providers in this picker. */
export function providerNamesCollide(refs: readonly Pick<ProviderRef, 'id' | 'name'>[]): boolean {
  const seen = new Map<string, string>();
  for (const ref of refs) {
    const name = foldName(ref.name);
    const previous = seen.get(name);
    if (previous !== undefined && previous !== ref.id) return true;
    seen.set(name, ref.id);
  }
  return false;
}

/** Pickers use served readable origins; a locator is never turned into a display-name suffix. */
export function providerPickerLabels(refs:readonly ProviderRef[],providers:readonly ProviderNameData[]=[],known:KnownPlaces=new Map()):ReadonlyMap<string,string> {
  const full=new Map(providers.map((provider)=>[provider.id,provider]));
  return resourceLabels(refs.map((ref)=>{
    const provider=full.get(ref.id);
    return provider ?? {id:ref.id,name:ref.name};
  }),known);
}
