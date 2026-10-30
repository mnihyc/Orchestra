/** What other screens ask of the Browser and VNC views: a browser or a display to bring to the front (from the row of the agent's tool), and an address to open in a browser of the environment (from a port's "Here in its browser").
 *
 * The view takes each request when it next draws for that environment, and lets it go: a request is
 * an act of the person, never a choice that is kept.
 */
import { create } from 'zustand';

export interface BrowserRequests {
  /** A browser to show in front, by its id. */
  front: string | null;
  /** A display to show in front, by its id. */
  display: string | null;
  /** An address to open, in the environment named, with the profile chooser. */
  open: { environmentId: string; url: string } | null;
  showBrowser: (id: string) => void;
  showDisplay: (id: string) => void;
  openAddress: (environmentId: string, url: string) => void;
  takeFront: () => void;
  takeDisplay: () => void;
  takeOpen: () => void;
}

export const useBrowserRequests = create<BrowserRequests>((set) => ({
  front: null,
  display: null,
  open: null,
  showBrowser: (id) => set({ front: id }),
  showDisplay: (id) => set({ display: id }),
  openAddress: (environmentId, url) => set({ open: { environmentId, url } }),
  takeFront: () => set({ front: null }),
  takeDisplay: () => set({ display: null }),
  takeOpen: () => set({ open: null }),
}));
