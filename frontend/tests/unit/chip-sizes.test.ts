/** Presentation choices persist with their actual owner and cannot expand a compact row into a card. */
import { afterEach, expect, it } from 'vitest';
import { chipSize, readChipSizes } from '@/features/people/chipSizes';
import { useWorkbench } from '@/features/workbench/store';
import { loadWorkspace } from '@/features/workbench/persistence';
import { buildTree } from '@/features/documents/blocks';
import { docToDrafts, orchSchema, treeToJson } from '@/features/documents/schema';
import type { DocumentBlock } from '@/generated/api-types';

afterEach(() => { useWorkbench.getState().leave(); });

it.each(['compact', 'prose', 'document'] as const)('keeps each explicit size in %s and applies its place default', (place) => {
  expect(chipSize(place, 'small')).toBe('small');
  expect(chipSize(place, 'medium')).toBe('medium');
  expect(chipSize(place, 'large')).toBe(place === 'compact' ? 'medium' : 'large');
  expect(chipSize(place)).toBe(place === 'compact' ? 'small' : place === 'prose' ? 'medium' : 'large');
});

it('keeps a device choice with its reader and restores defaults when the arrangement is reset', () => {
  useWorkbench.getState().enter('reader-one', 'thread');
  useWorkbench.getState().setChipSize('prose', 'large');
  useWorkbench.getState().setChipSize('compact', 'large');
  useWorkbench.getState().flush();
  expect(loadWorkspace('reader-one', 'thread').kept?.chipSizes).toEqual({ prose: 'large', compact: 'large' });
  useWorkbench.getState().leave();
  useWorkbench.getState().enter('reader-two', 'thread');
  expect(useWorkbench.getState().chipSizes).toEqual({});
  useWorkbench.getState().leave();
  useWorkbench.getState().enter('reader-one', 'thread');
  expect(useWorkbench.getState().chipSizes).toEqual({ prose: 'large', compact: 'large' });
  expect(chipSize('compact', useWorkbench.getState().chipSizes.compact)).toBe('medium');
  useWorkbench.getState().reset();
  useWorkbench.getState().flush();
  expect(loadWorkspace('reader-one', 'thread').kept?.chipSizes).toEqual({});
  expect(chipSize('compact')).toBe('small');
  expect(chipSize('prose')).toBe('medium');
  expect(chipSize('document')).toBe('large');
});

it('ignores invalid saved sizes and unrecognized kinds of place', () => {
  expect(readChipSizes({ compact: 'full-screen', prose: 'medium', document: 'large', other: 'small' })).toEqual({ prose: 'medium', document: 'large' });
  expect(readChipSizes(null)).toEqual({});
});

it.each(['small', 'medium', 'large'] as const)('preserves a document reference and media block saved as %s through editing', (display) => {
  const base = { document_id: 'doc', parent_id: null, removed: false, seq_created: 1, seq_updated: 1, updated_by: null, updated_at: '2026-10-25T00:00:00Z' };
  const blocks: DocumentBlock[] = [
    { ...base, id: 'paragraph', position: '1', type: 'paragraph', props: {}, content: [{ type: 'ref', kind: 'file', id: 'file', display }] },
    { ...base, id: 'image', position: '2', type: 'image', props: { file_id: 'file', display }, content: [] },
    { ...base, id: 'audio', position: '3', type: 'audio', props: { file_id: 'file', display }, content: [] },
    { ...base, id: 'video', position: '4', type: 'video', props: { file_id: 'file', display }, content: [] },
    { ...base, id: 'attachment', position: '5', type: 'file', props: { file_id: 'file', display }, content: [] },
  ];
  const state = orchSchema().nodeFromJSON(treeToJson(buildTree(blocks)));
  const drafts = docToDrafts(state);
  expect(drafts[0]?.content).toEqual([{ type: 'ref', kind: 'file', id: 'file', display }]);
  expect(drafts.slice(1).map((draft) => draft.props.display)).toEqual([display, display, display, display]);
});
