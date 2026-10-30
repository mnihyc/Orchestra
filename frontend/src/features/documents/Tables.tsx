/** Tables: growing by their edges, a header row and column, cells that span several, colours and column widths.
 *
 * A table grows with Tab in its last cell; the tools beside the caret add and take away a row or a
 * column, make the first row or column a header, merge a selection of cells and split a merged one,
 * and give the cells a colour. A column's edge is dragged to its width.
 */
import type { AnyExtension, Editor } from '@tiptap/core';
import { TableView } from '@tiptap/extension-table';
import type { Node as PmNode } from '@tiptap/pm/model';
import { CellSelection } from '@tiptap/pm/tables';
import type { EditorView } from '@tiptap/pm/view';
import { useEditorState } from '@tiptap/react';
import { BubbleMenu, type BubbleMenuProps } from '@tiptap/react/menus';
import { floatingClasses, floatingPlacement } from '@/ui/Popover';
import { Columns3, PaintBucket, Rows3, Table2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { OrchCell, OrchTable, OrchTableRow } from '@/features/documents/schema';
import { holdsFocus } from '@/features/documents/TextBar';
import { tintEntries } from '@/features/documents/tints';
import { Button } from '@/ui/Button';
import { Menu, type MenuEntry } from '@/ui/Menu';

/** The least width of a column, in pixels. */
const COLUMN_LEAST = 56;

/** The table's own view, which says whether its first row and column are a header. */
class OrchTableView extends TableView {
  constructor(node: PmNode, cellMinWidth: number, view: EditorView, attributes?: Record<string, unknown>) {
    super(node, cellMinWidth, view, attributes);
    this.mark(node);
  }

  override update(node: PmNode): boolean {
    const same = super.update(node);
    if (same) this.mark(node);
    return same;
  }

  private mark(node: PmNode): void {
    this.table.dataset.headerRow = String(node.attrs.header_row === true);
    this.table.dataset.headerColumn = String(node.attrs.header_column === true);
  }
}

/** The nodes of a table, drawn so that a column's edge can be dragged. */
export const tableViews: Readonly<Record<string, AnyExtension>> = {
  table: OrchTable.configure({ resizable: true, cellMinWidth: COLUMN_LEAST, lastColumnResizable: true, View: OrchTableView }),
  tableRow: OrchTableRow,
  tableCell: OrchCell,
};

/** The position of the table the selection is in. */
function tableAt(editor: Editor): { pos: number; node: PmNode } | null {
  const { $from } = editor.state.selection;
  for (let depth = $from.depth; depth > 0; depth -= 1) {
    if ($from.node(depth).type.name === 'table') return { pos: $from.before(depth), node: $from.node(depth) };
  }
  return null;
}

function setTableAttr(editor: Editor, name: string, value: unknown): void {
  const at = tableAt(editor);
  if (at === null) return;
  editor.view.dispatch(editor.state.tr.setNodeAttribute(at.pos, name, value));
}

/** The tools show while the caret is in a table or cells are selected. Held outside the component, as the text bar's are. */
const showInTable: NonNullable<BubbleMenuProps['shouldShow']> = ({ editor: current, element, view, state: pm }) =>
  current.isEditable && (view.hasFocus() || holdsFocus(element)) && tableAt(current) !== null && (pm.selection.empty || pm.selection instanceof CellSelection);
const TOOLS_PLACEMENT = { placement: 'top-start', offset: 8, ...floatingPlacement } as const;

/** The tools of a table, beside the caret while it is in one. */
export function TableTools({ editor }: { editor: Editor }) {
  const { t } = useTranslation();
  const state = useEditorState({
    editor,
    selector: ({ editor: current }) => {
      const at = tableAt(current);
      const selection = current.state.selection;
      return {
        inTable: at !== null,
        headerRow: at?.node.attrs.header_row === true,
        headerColumn: at?.node.attrs.header_column === true,
        cells: selection instanceof CellSelection,
        canMerge: current.can().mergeCells(),
        canSplit: current.can().splitCell(),
        editable: current.isEditable,
      };
    },
  });
  const run = (name: string) => () => (editor.chain().focus() as unknown as Record<string, () => { run: () => boolean }>)[name]?.().run();
  const rows: MenuEntry[] = [
    { id: 'row-before', label: t('documents.table.addRowBefore'), onSelect: run('addRowBefore') },
    { id: 'row-after', label: t('documents.table.addRowAfter'), onSelect: run('addRowAfter') },
    { id: 'row-delete', label: t('documents.table.deleteRow'), danger: true, onSelect: run('deleteRow') },
  ];
  const columns: MenuEntry[] = [
    { id: 'column-before', label: t('documents.table.addColumnBefore'), onSelect: run('addColumnBefore') },
    { id: 'column-after', label: t('documents.table.addColumnAfter'), onSelect: run('addColumnAfter') },
    { id: 'column-delete', label: t('documents.table.deleteColumn'), danger: true, onSelect: run('deleteColumn') },
  ];
  const cell: MenuEntry[] = [
    { id: 'merge', label: t('documents.table.merge'), disabled: state.canMerge ? false : t('documents.table.mergeWhy'), onSelect: run('mergeCells') },
    { id: 'split', label: t('documents.table.split'), disabled: state.canSplit ? false : t('documents.table.splitWhy'), onSelect: run('splitCell') },
    { type: 'separator', id: 'cell-colour' },
    { type: 'label', id: 'label-colour', label: t('documents.table.colour') },
    ...tintEntries('cell-color', t('documents.table.colour'), null, (tint) => editor.chain().focus().setCellAttribute('color', tint).run()),
    { type: 'label', id: 'label-fill', label: t('documents.table.fill') },
    ...tintEntries('cell-background', t('documents.table.fill'), null, (tint) => editor.chain().focus().setCellAttribute('background', tint).run()),
  ];
  const table: MenuEntry[] = [
    { type: 'check', id: 'header-row', ...choiceOption(t, 'tableHeader.row'), checked: state.headerRow, onCheckedChange: (on) => setTableAttr(editor, 'header_row', on) },
    { type: 'check', id: 'header-column', ...choiceOption(t, 'tableHeader.column'), checked: state.headerColumn, onCheckedChange: (on) => setTableAttr(editor, 'header_column', on) },
    { type: 'separator', id: 'table-delete-sep' },
    { id: 'table-delete', label: t('documents.table.delete'), danger: true, onSelect: run('deleteTable') },
  ];
  if (!state.editable) return null;
  return (
    <BubbleMenu
      editor={editor}
      pluginKey="orchTableTools"
      shouldShow={showInTable}
      options={TOOLS_PLACEMENT}
    >
      <div role="toolbar" aria-label={t('documents.table.tools')} className={`flex flex-wrap items-center gap-1 overflow-y-auto rounded-lg border border-border bg-surface-raised p-1 shadow-md ${floatingClasses}`}>
        <Menu label={t('documents.table.row')} trigger={<Button size="sm" variant="ghost" icon={<Rows3 aria-hidden />}>{t('documents.table.row')}</Button>} entries={rows} />
        <Menu label={t('documents.table.column')} trigger={<Button size="sm" variant="ghost" icon={<Columns3 aria-hidden />}>{t('documents.table.column')}</Button>} entries={columns} />
        <Menu label={t('documents.table.cells')} trigger={<Button size="sm" variant="ghost" icon={<PaintBucket aria-hidden />}>{t('documents.table.cells')}</Button>} entries={cell} />
        <Menu label={t('documents.table.tools')} trigger={<Button size="sm" variant="ghost" icon={<Table2 aria-hidden />}>{t('documents.table.tools')}</Button>} entries={table} />
      </div>
    </BubbleMenu>
  );
}
