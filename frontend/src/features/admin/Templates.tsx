/** Both template kinds through the shared, authorized system inventory and editor. */
import { useState } from 'react';
import { ResourceListScope } from '@/features/resources/ResourceList';
import { TemplateList, type TemplatesSearch } from '@/features/resources/templates/TemplateList';

export function Templates() {
  const [search, setSearch] = useState<TemplatesSearch>({});
  return <ResourceListScope scope="system" embedded><TemplateList search={search} onSearch={(patch) => setSearch((current) => ({ ...current, ...patch }))} /></ResourceListScope>;
}
