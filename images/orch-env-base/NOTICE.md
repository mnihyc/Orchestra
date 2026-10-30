# Software in orch-env-base

The image combines software under different terms. `LicenseRef-Orch-Env-Base`
in its image label refers to this notice; it does not grant one licence for
everything in the image. The Dockerfile and this notice are Apache-2.0.

| Component | Terms and notices |
|-----------|-------------------|
| Google Chrome for Testing, downloaded by Playwright | Google's executable distribution of Chrome, under the [Google Terms of Service](https://policies.google.com/terms) and [Chrome Additional Terms](https://www.google.com/chrome/terms/). It has no open source licence for the executable as a whole. The licences of its included open source components are available in `chrome://credits`; its terms are also in `chrome://terms`. It is not an Apache-2.0 distribution or the Chromium project. |
| Node.js | MIT for Node.js itself and separate licences for its incorporated dependencies. The complete bundled notice is `/usr/local/LICENSE`; [the upstream licence](https://github.com/nodejs/node/blob/main/LICENSE) identifies those components and their terms. |
| Playwright's command line (`@playwright/cli`) and Playwright | Apache-2.0. Their installed packages, including dependencies, are under `/usr/local/lib/node_modules/@playwright/cli`; each package's `LICENSE` and `NOTICE`, where supplied, apply to that package. [Playwright source and licence](https://github.com/microsoft/playwright). |
| FFmpeg, downloaded by Playwright for recording | LGPL-2.1-or-later. The downloaded build keeps its licence at `/opt/ms-playwright/ffmpeg-*/COPYING.LGPLv2.1`; its `ffmpeg-linux -L` command reports the build's terms and configuration. Preserve that licence and make the corresponding source for the distributed build available. [Playwright's FFmpeg build recipe](https://github.com/microsoft/playwright/tree/main/browser_patches/ffmpeg) identifies its source and build options. |
| TigerVNC, including Xvnc | GPL-2.0-or-later for TigerVNC's own code, with separately licensed X server and library code. Debian's complete copyright and licence record is `/usr/share/doc/tigervnc-standalone-server/copyright`, with records for its other packages beside it. [TigerVNC source](https://github.com/TigerVNC/tigervnc). |
| openbox | GPL-2.0-or-later, with the exceptions and third-party components described in `/usr/share/doc/openbox/copyright`. [openbox source](https://github.com/danakj/openbox). |
| Fonts | Noto CJK, Noto Color Emoji and Liberation use SIL Open Font License 1.1. DejaVu includes the Bitstream Vera licence and public-domain changes. GNU Unifont includes GPL terms with a font embedding exception; individual files and contributors have their own records. The authoritative records are `/usr/share/doc/fonts-noto-cjk/copyright`, `/usr/share/doc/fonts-noto-color-emoji/copyright`, `/usr/share/doc/fonts-liberation/copyright`, `/usr/share/doc/fonts-dejavu-core/copyright` and `/usr/share/doc/fonts-unifont/copyright`. |
| Debian packages and the distribution | Each package keeps its copyright and licence record under `/usr/share/doc/<package>/copyright`. Licence texts shared by packages are under `/usr/share/common-licenses`. There is no single licence covering all packages. [Debian copyright records](https://www.debian.org/doc/debian-policy/ch-docs.html#copyright-information). |

The pinned versions and download locations are listed in the image README.
The source of Debian packages is available from Debian's source archives and
the upstream projects; versions installed in this image can be listed with
`dpkg-query -W`. Keep the bundled licence and copyright files when distributing
the image, and apply each component's own terms to its redistribution.
