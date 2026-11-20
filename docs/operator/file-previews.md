# Set file preview limits

Choose how large a file preview may be and how images reach agents. At the end previews fit the memory you give the API, and agents get images in the way their model supports.

File cards show disposable, bounded previews. A preview is not a new file and needs no migration: the original stays in its normal blob store with its current document, chat, thread or environment access rules. People see the cards as described in [Files and attachments](../user/files-and-attachments.md).

## Before you start

You need to be a system administrator. Search the keys below in the settings finder. The preview and viewer settings belong to the system. They are defaults you can change, not fixed ceilings.

## 1. Set the preview limits

| Setting | Shipped default | What it controls |
|---|---|---|
| `files.preview_source_bytes` | 16 MiB | Original bytes admitted for image decoding or bounded workspace acquisition |
| `files.preview_pixels` | 40,000,000 | Decoded image pixels admitted before any allocation |
| `files.preview_edge` | 512 px | The thumbnail's longest edge; small originals are not enlarged |
| `files.preview_bytes` | 256 KiB | Byte budget of an encoded PNG or a source excerpt |
| `files.preview_cache_bytes` | 64 MiB | The derived-response cache; 0 turns this cache off |
| `files.viewer_bytes` | 64 MiB | Original media bytes admitted into a browser viewer |
| `references.preview_lines` | 2 | Context of reference excerpts; ordinary file cards show at most five lines at this default |

1. Change the setting at the system.
2. A change applies to the requests after it.

Text cards also keep the reference card's 280-character limit, so one long line cannot fill a card with the whole text.

**The cache.** The derived-response cache lives in the API's memory as a least-recently-used cache bounded in bytes. An entry is identified by the content hash, the MIME kind and the effective preview settings, so equivalent content reuses the derived bytes. A current owner check and a check that the original exists come before serving them. Restarting the API drops the cache. Do not back it up; back up the database and the original blobs as [Back up and restore](backup-and-restore.md) says.

## What gets a preview

- PNG, JPEG, GIF, WebP, BMP and ICO make static PNG thumbnails. EXIF orientation is applied before resizing, and an animated file uses its first frame.
- SVG geometry is drawn without outside resources or host fonts. An SVG with an embedded raster image stays unavailable until its nested decoding can be admitted safely.
- HEIC, HEIF, AVIF and TIFF show the ordinary unavailable or download state.
- PDF and video cards show a kind icon, and opening uses the browser's own PDF or media viewer.
- ZIP counts and the length of fixed-header PCM WAV files are read when available. An unknown detail shows a dash.
- A card never downloads a whole original into the browser to make a thumbnail.

Original reads carry `nosniff` and a sandbox policy. Active HTML, XML, SVG and multipart content is an inert download. Authorized originals use private cache revalidation, and derived previews use private no-store. Cached preview bytes never grant access to a new reader and do not survive a removed original as readable content.

## 2. Choose how images reach agents

`files.native_image_bytes` is 20 MiB by default, the same as the whole-image upload allowance. It inherits from the system through the places of the thread down to its topic, and can be locked.

- **Zero** sends every image by path.
- **A positive value** passes an image up to and including that size as the model's own image input, but only when both the model and the agent program declare image input.
- Every attached image is also saved as a file. Other kinds of file use their verified receiving path, kind and byte count.
- No agent program declares PDF input today, so PDFs are delivered as files.

The effective policy is checked when an input starts or is steered. A delivery, permission or integrity failure still holds the input. A model's lack of image support alone does not.

## If it does not work

| You see | What to do |
|---|---|
| A card shows no thumbnail for a large image | The original is over `files.preview_source_bytes` or `files.preview_pixels`. Raise the setting, or leave it: the original is still downloadable |
| A card for a TIFF or HEIC is unavailable | These formats are not decoded. Download the original |
| An agent receives a path and not the image | The image is over `files.native_image_bytes`, the value is 0, or the model or program does not declare image input |
| The API's memory grows with previews | Lower `files.preview_cache_bytes`, or set it to 0 |
