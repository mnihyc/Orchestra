/** Repeated locator requests move the existing editor selection without replacing its unsaved text. */
import { act, screen } from '@testing-library/react';
import { createRef } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { CodeView, type CodeViewHandle } from '@/ui/CodeView';
import { show } from './support';

afterEach(() => vi.unstubAllGlobals());
it('an explicit repeated line request preserves the mounted editor and dirty draft', () => {
  // No geometry is claimed in jsdom; browser checks cover actual scrolling and reachability.
  vi.stubGlobal('requestAnimationFrame', vi.fn(() => 0));
  const editor = createRef<CodeViewHandle>(), dirty = vi.fn<(changed: boolean) => void>();
  const text = 'first\nsecond\nthird\nfourth';
  const pane = show(<CodeView ref={editor} text={text} version="original" label="Own code" line={2} lineRequest={1} onDirtyChange={dirty} />);
  const body = screen.getByRole('textbox', { name: 'Own code' });
  expect(editor.current?.getSelection().fromLine).toBe(2);
  act(() => { editor.current?.setText(`${text}\nunsaved fifth`); editor.current?.goToLine(4); });
  expect(editor.current?.getSelection().fromLine).toBe(4);
  pane.rerender(<CodeView ref={editor} text={text} version="original" label="Own code" line={2} lineRequest={1} onDirtyChange={dirty} />);
  expect(editor.current?.getSelection().fromLine).toBe(4);
  pane.rerender(<CodeView ref={editor} text={text} version="original" label="Own code" line={2} lineRequest={2} onDirtyChange={dirty} />);
  expect(editor.current?.getSelection().fromLine).toBe(2);
  expect(editor.current?.getText()).toBe(`${text}\nunsaved fifth`);
  expect(screen.getByRole('textbox', { name: 'Own code' })).toBe(body);
  expect(dirty).toHaveBeenLastCalledWith(true);
});

it.each(['Own code', '代码'])('opens an inclusive line selection in %s without replacing unsaved text', (label) => {
  vi.stubGlobal('requestAnimationFrame', vi.fn(() => 0));
  const editor = createRef<CodeViewHandle>();
  const text = 'first\nsecond\nthird\nfourth';
  const pane = show(<CodeView ref={editor} text={text} version="original" label={label} line={2} endLine={3} />);
  expect(editor.current?.getSelection()).toMatchObject({ fromLine: 2, toLine: 3, text: 'second\nthird' });
  act(() => { editor.current?.setText(`${text}\nunsaved fifth`); });
  pane.rerender(<CodeView ref={editor} text={text} version="original" label={label} line={3} endLine={100} />);
  expect(editor.current?.getSelection()).toMatchObject({ fromLine: 3, toLine: 5, text: 'third\nfourth\nunsaved fifth' });
  expect(editor.current?.getText()).toBe(`${text}\nunsaved fifth`);
});
