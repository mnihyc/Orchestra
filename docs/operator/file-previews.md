# File previews and agent delivery

File cards use disposable, bounded previews. They do not add a new authoritative file or require a migration.
The original remains in its normal blob store and keeps its current document, chat, thread or environment access rules.

## Preview settings

Search these keys in the settings finder. The preview and viewer settings belong to System.
They are defaults an operator can change, rather than fixed product ceilings.

| Setting | Shipped default | What it controls |
|---|---|---|
| `files.preview_source_bytes` | 16 MiB | Original bytes admitted for image decoding or bounded workspace acquisition |
| `files.preview_pixels` | 40,000,000 | Decoded image pixels admitted before allocation |
| `files.preview_edge` | 512 px | Thumbnail's longest edge; small originals are not enlarged |
| `files.preview_bytes` | 256 KiB | Encoded PNG or source excerpt byte budget |
| `files.preview_cache_bytes` | 64 MiB | Derived response cache; 0 turns this cache off |
| `files.viewer_bytes` | 64 MiB | Original media bytes admitted into a browser viewer |
| `references.preview_lines` | 2 | Context for reference excerpts; ordinary file cards show at most five lines at this default |

Text cards also use the reference card's 280-character bound. A long single line cannot fill a hidden card with the whole text.
The API cache is a byte-bounded LRU in memory. Its identity includes the content hash, MIME kind and effective preview settings.
Equivalent content reuses derived bytes. A current owner check and original-existence check precede serving them.
Changing a preview setting affects subsequent requests. Restarting the API drops this disposable cache; do not include it in a backup.
Keep the database and original blobs as described in [Backup and restore](backup-and-restore.md).

## Formats and serving

PNG, JPEG, GIF, WebP, BMP and ICO produce static PNG thumbnails. EXIF orientation is applied before resizing.
Animated files use the first frame. SVG geometry is rasterized without external resources or host fonts.
SVGs with embedded raster images remain unavailable until their nested decoding can be admitted safely.
HEIC, HEIF, AVIF and TIFF use the ordinary unavailable/download state.

PDF and video cards use kind icons; opening uses the browser's existing PDF or media viewer.
ZIP counts and fixed-header PCM WAV lengths are read when available. Unknown details display a dash.
Cards never download a whole original into the browser to make a thumbnail.

Original reads use `nosniff` and a sandbox policy. Active HTML, XML, SVG and multipart content is an inert download.
Authorized originals use private cache revalidation. Derived preview responses use private no-store.
Cached preview bytes do not grant access to a new reader or survive a removed original as readable content.

## Native model inputs

`files.native_image_bytes` defaults to 20 MiB, matching the existing whole-image upload allowance.
It inherits from System through the thread's places to its topic and can be locked through the ordinary settings rules.
Zero sends every image by path. A positive value admits native images up to and including that size,
only when both the model and harness declare image input support.

Every attached image is also materialized. Other file kinds use their verified receiving path, kind and byte count.
No current harness declares native PDF input, so PDFs use file delivery.
A delivery, permission or integrity failure still holds the input; a model's lack of image support alone does not.
The effective policy is checked when an input starts or is steered.

For the visible card states and stored chip choices, see [Files and attachments](../user/files-and-attachments.md).
