---
title: Files and attachments
screens: /thread/$threadId, /chat/$chatId, /place-chat/$nodeId, /org/$slug/messages, /doc/$documentId, /workbench, /search, /inbox, /resources/environments/$id, /inspect/$kind/$id
keywords: files, attachments, image, photo, PDF, audio, video, preview, chip, card, Show as, Small, Medium, Large, download, reference, agent, 文件, 附件, 预览, 标签大小
heading_keywords.previews-and-chip-sizes: thumbnail, Show as, Small, Medium, Large, right click, menu key, long press, document, workbench
heading_keywords.open-and-download: viewer, image, PDF, audio, video, code, Markdown, archive, original, reduced motion
heading_keywords.files-for-an-agent: model, native image, path, size, shared document, download_shared
heading_keywords.preview-states: loading, unavailable, gone, private, too large
---

# Files and attachments

Messages, agent tools, documents, Files, search and Inbox use the same file chip and card.
Open the card by hovering, focusing or pressing the chip. A picture has a small thumbnail when a preview is available.
In Files, selecting a name opens its ordinary code view; its card remains available on hover or focus.

## Previews and chip sizes

The card shows a preview, the name, the size and the details that are known. Unknown details read **–**.
Text and code show a short excerpt. Archives show their entry count when it is available.
Audio and video show their length when it can be read. A kind icon stands in for a picture that is unavailable.

Right-click a chip, press the menu key or Shift+F10, or long-press it. Choose **Show as**:

| Size | What appears |
|---|---|
| Small | Kind glyph and name |
| Medium | Glyph, name and a muted hint or size |
| Large | A card in the flow, with its preview, title, first description line, owner and last change where known |

Compact rows start Small. Messages and other prose start Medium. A reference standing alone in a document starts Large.
A Large choice in a compact row appears as Medium, so the row stays compact.

In a document you may edit, this choice is saved with that reference or media block. Everyone sees that saved choice.
A read-only document explains why the size cannot be changed. Elsewhere, the choice belongs to your workbench on this device,
for that kind of place. Resetting the workbench restores its defaults.

## Open and download

Press a thumbnail or choose **Open** in its card. Attachments use the same viewers as workspace files.
Images can fit the view or show at their actual size. PDFs have page, zoom and text-search controls.
Audio and video use their player controls. Text, code, Markdown, differences and archives use their own viewers.
Other files can be downloaded or opened as text. Attachment views are read-only; edit a writable workspace file in its code view.

Previews use a small copy. **Download** gives you the original bytes. Opening a viewer reads the original only when needed.
With reduced motion, an animated image stays on its static preview until you choose **Play animation**.
An external media address in a document waits until you choose to play it.
Its card shows the host and filename on one line. Hover for the whole address.

**Copy reference** gives you a link to paste into a message or document. A workspace reference can include its selected line.
An unfinished draft or tool artifact without a saved reference or receiving path explains why Copy reference is unavailable.

## Files for an agent

Attach files in the composer and send the message. An image reaches the model directly when the model and harness support it
and the image fits the configured native-image allowance. It is also written into the thread's directory.
Other files reach the agent by their checked workspace path, kind and size.

**This model receives the image as a file** means the image will reach the agent by path. You can send it or choose another model.
A real file-delivery failure holds the input and names the file and reason.

Files in a shared document remain file references. The agent follows them with `download_shared` to get their originals.
Forwarding and the files selected in a fork, switch, move or export carry originals, not thumbnails. Their plans and current access still apply.

## Preview states

- A skeleton means the preview is loading. Previews are requested when they become visible.
- **Preview unavailable** means this kind or file could not produce a preview. Download is still available.
  HEIC, HEIF, AVIF and TIFF currently use this fallback. PDF and video cards use their kind icon.
- **File no longer exists** means the original is gone. Open and Download are unavailable.
- **Private reference** hides the name and card when you may not read the file.
- **Too large to preview** offers Download. The preview limits leave the original unchanged.

See [Work and collaboration](workflows.md) for sharing and [Finding things](finding-things.md) for search and the finder.
