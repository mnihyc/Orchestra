/** An org's contents use the reader's complete hierarchy, starting at that org. */
import { EverythingBrowser } from '@/features/everything/Everything';

export function ContentsTree({ orgId, q }: { orgId: string; q: string }) {
  return <EverythingBrowser parent={orgId} q={q} />;
}
