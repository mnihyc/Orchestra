# VNC view

The displays of an environment, and one of them drawn by noVNC (`@novnc/novnc` 1.7.0, MPL-2.0, used as
it is and fetched only when the view is drawn), beside a thread (the view `vnc`) and as the VNC tab of the
environment's page. What is real: the list kept current by the environment's feed of displays; "Open a
display…" with a name and a size; Close in its own words for a display Orch opened ("What shows on it ends
with it") and one an agent announced ("It is no longer shown here. Its program runs on"); each way of being
off and nothing found; the viewer on a socket this view opens with its ticket and subprotocol, told to send
nothing until "Work in it", which is a socket of its own; why a socket ended, from its close frame; the
line a program is started with, to copy; the sentence about what is copied on a display Orch did not open;
Paste text…, Send Ctrl+Alt+Del and Fit to this panel; the quality lowered when updates fall behind and
raised when they keep up, with "Reduced picture" (`viewer.ts` says how it is measured).

By keyboard the controls round the viewer are reached as usual; inside, the viewer's canvas takes the keys
while one works in it, and Ctrl+Alt+Esc gives them back. The fake world's displays (`tests/doubles/displays.ts`)
speak RFB 3.8 with raw pixels; the real server has not been tried from here.

- `viewer.ts`: the socket's address, why it ended, and how the quality follows the updates.
- `Viewer.tsx`: noVNC around a socket of the view's own.
- `Displays.tsx`: the list, the dialogs, and the display in front.
- `View.tsx`: the view's registration beside a thread.
- `novnc.d.ts`: the part of noVNC's API the view uses, as its documentation gives it.
