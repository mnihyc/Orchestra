# Software in orch-env-dev

The image is the Default environment image (see `/usr/share/doc/orch-env-base/NOTICE.md` for its software) with the
software below added. `LicenseRef-Orch-Env-Dev` in its image label refers to this notice and to that one; it does not
grant one licence for everything in the image. The Dockerfile, `orch-dockerd` and this notice are Apache-2.0.

| Component | Terms and notices |
|-----------|-------------------|
| Docker Engine, the Docker command line, buildx and compose | Apache-2.0. Each package's licence and notices are under `/usr/share/doc/<package>/`; [Docker Engine source and licence](https://github.com/moby/moby). |
| containerd | Apache-2.0. [containerd source and licence](https://github.com/containerd/containerd). |
| QEMU | GPL-2.0-or-later for QEMU's own code, with separately licensed components; Debian's complete record is `/usr/share/doc/qemu-system-x86/copyright`, with records for the other QEMU packages beside it. [QEMU source](https://www.qemu.org/). |
| Build tools and development files (cmake, pkg-config, autoconf, automake, libtool, OpenSSL, zlib, libffi) | Each package keeps its copyright and licence record under `/usr/share/doc/<package>/copyright`. |

The source of Debian packages is available from Debian's source archives; the versions installed in this image can be
listed with `dpkg-query -W`. Keep the bundled licence and copyright files when distributing the image, and apply each
component's own terms to its redistribution.
