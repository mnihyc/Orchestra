/** The rule for where a floating layer above or below what opened it stands: the side that holds it, kept while it shows enough, left for the roomier one when it cannot. */
import { expect, it } from 'vitest';
import { layerRoom, floatingRoom } from '@/ui/floating';

// A 900 px screen; a field whose bottom edge sits at `bottom`. The gap is 4 px and the screen's edge 8 px.
const viewport = { top: 0, bottom: 900 };
const field = (bottom: number) => ({ top: bottom - 36, bottom });
const room = (bottom: number, natural: number, least: number, now?: 'above' | 'below') =>
  layerRoom({ anchor: field(bottom), viewport, natural, least, gap: 4, ...(now === undefined ? {} : { now }) });

it('opens below when all of it fits there, and above when it fits only there', () => {
  expect(room(200, 400, 160)).toEqual({ above: false, height: 900 - 200 - 12 });
  expect(room(800, 400, 160)).toEqual({ above: true, height: 800 - 36 - 12 });
});

it('opens on the larger side when it fits on neither, and below when they are alike', () => {
  expect(room(800, 3000, 160)).toEqual({ above: true, height: 752 });
  expect(room(200, 3000, 160)).toEqual({ above: false, height: 688 });
  expect(room(456, 3000, 160)).toEqual({ above: false, height: 432 });
});

it('stays where it stands while that side shows the rows it promises, however its content changes', () => {
  // Below a field with 240 px of room: 400 px of content, then 3000, then 60.
  for (const natural of [400, 3000, 60]) expect(room(648, natural, Math.min(natural, 160), 'below')).toEqual({ above: false, height: 240 });
  // Above, with the whole screen's room beside it, it does not go back below because the list is shorter.
  expect(room(800, 60, 60, 'above')).toEqual({ above: true, height: 752 });
  expect(room(110, 60, 60, 'above')).toEqual({ above: true, height: 62 });
});

it('leaves a side that shows less than it promises for the one with more room', () => {
  // The list arrived after the popup opened below a field low on the screen: 82 px of room, 530 px of content, 162 px promised.
  expect(room(806, 530, 162, 'below')).toEqual({ above: true, height: 758 });
  // The same the other way: above a field at the top of the screen.
  expect(room(100, 530, 162, 'above')).toEqual({ above: false, height: 788 });
});

it('stays when the other side has no more room to give, and holds a short list whole before it leaves', () => {
  // A 300 px screen: 138 px below the field, 102 above, 162 px promised. Neither gives it; the larger one is below.
  const small = (now: 'above' | 'below') => layerRoom({ anchor: field(150), viewport: { top: 0, bottom: 300 }, natural: 3000, least: 162, gap: 4, now });
  expect(small('below')).toEqual({ above: false, height: 138 });
  expect(small('above')).toEqual({ above: false, height: 138 });
  // A list of 2 rows (90 px) is promised whole: 80 px of room below is less than that, and above has more.
  expect(room(808, 90, 90, 'below')).toEqual({ above: true, height: 760 });
});

it('keeps the hint rule for what shows above its trigger, and lets a dropdown name below', () => {
  expect(floatingRoom({ top: 400, bottom: 436 }, 900, 100).above).toBe(true);
  expect(floatingRoom({ top: 400, bottom: 436 }, 900, 100, 'below').above).toBe(false);
  expect(floatingRoom({ top: 770, bottom: 806 }, 900, 300, 'below', 4)).toEqual({ above: true, height: 758 });
});

it('opens a list below whenever below shows the rows it promises, even where all of it would fit above', () => {
  // A field in the lower half of a dialog (bottom at 530 of 900): 358 px below, 482 above, a 470 px list promising 160.
  // As VS Code's lists do, it opens below and scrolls; it no longer climbs over the dialog's title because above would hold it whole.
  expect(layerRoom({ anchor: field(530), viewport, natural: 470, least: 160, gap: 4, list: true })).toEqual({ above: false, height: 358 });
  // Below too little for the rows it promises: where all of it fits, as before.
  expect(layerRoom({ anchor: field(800), viewport, natural: 470, least: 160, gap: 4, list: true })).toEqual({ above: true, height: 752 });
  // A card has no rows to scroll to: it still opens where it is seen whole.
  expect(layerRoom({ anchor: field(530), viewport, natural: 470, least: 128, gap: 4 })).toEqual({ above: true, height: 482 });
});
