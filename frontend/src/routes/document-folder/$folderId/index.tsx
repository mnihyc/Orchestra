/** A stable document-folder address works without opening its hierarchy node. */
import { usePageParams } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';
import { DocumentFolderPage } from '@/features/documents/DirectoryBrowser';

export const Route = createFileRoute('/document-folder/$folderId/')({ staticData: { screen: 'page' }, component: Screen });
function Screen() { return <DocumentFolderPage id={usePageParams(Route).folderId} />; }
