/** Both entrances share the same native related search, page and recovery behavior. */
import { RelatedPanel } from '@/features/inspector/Related';
export function RelatedTab({ documentId, seq, enabled }: { documentId: string; seq: number; enabled: boolean }) {
  return <RelatedPanel documentId={documentId} seq={seq} enabled={enabled} />;
}
