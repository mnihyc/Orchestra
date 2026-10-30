/** A document. Loader, permission handling and search schema are pending.
 *
 * The screen is a document's page of blocks, written in place. The search holds a block to open at (a
 * link to a block opens at that block) and an earlier version to read: `version` shows the document as
 * it stood then, read-only.
 */
import { usePageParams, usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { DocumentPage } from '@/features/documents/Document';
import { GraphPage } from '@/features/links/GraphPage';

interface DocumentSearch {
  block?: string;
  version?: number;
  new?: boolean;
  file?: string;
  line?: number;
  /** The graph of what the document is linked with, in place of the document, and how it is drawn. */
  graph?: boolean;
  depth?: number;
  kinds?: string;
  meaning?: boolean;
  center?: string;
}

export const Route = createFileRoute('/doc/$documentId/')({
  staticData: { screen: 'document' },
  validateSearch: (search: Record<string, unknown>): DocumentSearch => {
    const out: DocumentSearch = {};
    if (typeof search.block === 'string' && search.block !== '') out.block = search.block;
    const version = Number(search.version);
    if (Number.isInteger(version) && version > 0) out.version = version;
    if (search.new === true || search.new === 'true') out.new = true;
    if (typeof search.file === 'string' && search.file !== '') out.file = search.file;
    const line = Number(search.line);
    if (Number.isSafeInteger(line) && line > 0) out.line = line;
    if (search.graph === true || search.graph === 'true' || search.graph === 1 || search.graph === '1') out.graph = true;
    const depth = Number(search.depth);
    if (depth === 1 || depth === 2 || depth === 3) out.depth = depth;
    if (typeof search.kinds === 'string' && search.kinds !== '') out.kinds = search.kinds;
    if (search.meaning === true || search.meaning === 'true') out.meaning = true;
    if (search.meaning === false || search.meaning === 'false') out.meaning = false;
    if (typeof search.center === 'string' && search.center !== '') out.center = search.center;
    return out;
  },
  component: DocumentRoute,
});

function DocumentRoute() {
  const { documentId } = usePageParams(Route);
  const { block, version, new: fresh, file, line, graph, depth, kinds, meaning, center } = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  if (graph === true) {
    return <GraphPage documentId={documentId} search={{ depth, kinds, meaning, center }}
      onSearch={(change) => void navigate({ search: (before) => ({ ...before, ...('depth' in change ? { depth: change.depth } : {}), ...('kinds' in change ? { kinds: change.kinds } : {}), ...('meaning' in change ? { meaning: change.meaning } : {}), ...('center' in change ? { center: change.center } : {}) }), replace: true })} />;
  }
  return (
    <DocumentPage
      documentId={documentId}
      block={block}
      file={file}
      line={line}
      onFileClosed={() => void navigate({ search: (before) => ({ ...before, file: undefined, line: undefined }), replace: true })}
      version={version}
      fresh={fresh === true}
      onOffered={() => void navigate({ search: (before) => ({ ...before, new: undefined }), replace: true })}
      onVersion={(next) => void navigate({ search: (before) => ({ ...before, version: next }), replace: true })}
    />
  );
}
