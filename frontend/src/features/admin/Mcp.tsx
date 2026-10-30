/** MCP presets through the shared, authorized system inventory and editor. */
import { ResourceListScope } from '@/features/resources/ResourceList';
import { ResourceNames } from '@/features/resources/ResourceNames';

export function Mcp() {
  return <ResourceListScope scope="system" embedded><ResourceNames kind="mcp" /></ResourceListScope>;
}
