# Default environment image

The image the Default template's environments are made from: Debian (the distribution the stack runs), an
`agent` account with the ids the template names (1000), `/workspace` and `/home/agent` for the two volumes an
environment keeps, and what a coding agent reaches for first: a shell, git, `ripgrep`, a C compiler, Python,
curl and the usual small tools. Beside that it holds three things that a smaller machine goes without:

- **Root without a password**: `sudo` for the `agent` account, so that whoever works there, an agent or a person
  at a shell, installs what the work needs.
- **A browser**, with Playwright's command line that agents drive it with and the Node.js that runs on, the
  libraries the browser draws with, and fonts for Chinese, Japanese, Korean and emoji.
- **A display**: TigerVNC's `Xvnc` and a small window manager (`openbox`), so that a screen can be opened and a
  browser can have a window on it.

It holds nothing of Orch's: the connector and the harnesses come into a unit from the node's tool cache,
read-only, so any image works and this is the one that is seeded. A team that needs less makes a template from
a smaller image; one that needs more, from an image of its own that starts `FROM` this one (and gets the
variables below with it).

It is large for that: about 1.6 GiB unpacked (Docker lists it as 2.3 GB; the 505 MiB it was without these
three things is its first third), and 650 MB to pull. The browser and its video helper are about 420 MB of it,
Node.js with the command line 240 MB, the display 340 MB (170 MB of that is the software OpenGL with its LLVM that
Debian's `libgl1` asks of any X server), the fonts and the browser's libraries 150 MB.

## What it holds

| Item | Version | Where | Pinned by (build argument) | Label |
|------|---------|-------|----------------------------|-------|
| Node.js | 24.21.0, the long-term-support line "Krypton", maintained to April 2028 | `/usr/local/bin/node`, `npm`, `npx`, `corepack` | `NODE_VERSION` and its SHA-256 for each processor | `orch.image.node` |
| Playwright's command line, `@playwright/cli` | 0.1.22 | `/usr/local/bin/playwright-cli`, in `/usr/local/lib/node_modules/@playwright/cli` | `PLAYWRIGHT_CLI_VERSION` | `orch.image.playwright-cli` |
| The Playwright inside it | 1.64.0-alpha-1790635538000 | below the command line's `node_modules` | `PLAYWRIGHT_VERSION` | `orch.image.playwright` |
| The browser: Chrome for Testing, installed by the command line's own installer | 155.0.8059.12, revision 1247 | `/opt/ms-playwright/chromium-1247/chrome-linux64/chrome` (`chrome-linux-arm64` on aarch64) | `BROWSER_VERSION`, `BROWSER_REVISION` | `orch.image.browser` (`chromium`), `orch.image.browser.version`, `orch.image.browser.revision` |
| TigerVNC | 1.12.0 (Debian's `1.12.0+dfsg-8`) | `/usr/bin/Xvnc` | `TIGERVNC_VERSION` | `orch.image.tigervnc` |
| openbox | 3.6.1 | `/usr/bin/openbox` | `OPENBOX_VERSION` | `orch.image.openbox` |
| sudo | Debian's (1.9.13p3 today); not pinned, so that its fixes arrive | `/usr/bin/sudo` | | |

The pins are build arguments of the [Dockerfile](Dockerfile), each with its version as the default. The build checks
every one against what was installed and stops where they differ: Node.js against its hash and its own version,
the command line and the Playwright it depends on against `npm`, the browser against what it says it is, the
VNC server and the window manager against what they print. The labels are written from the same arguments
(`org.opencontainers.image.version` is the release's; `org.opencontainers.image.base.name` the distribution's
image), and `cargo xtask image` and `cargo xtask package` read them back and refuse an image whose labels do not
say what the recipe pins. A newer command line asks for another browser, so the command line, its Playwright and
the browser move together.

## What the image promises

What follows is what the parts that start browsers and displays in a unit can rely on. All of it is checked by
[the shipped-image scenario](../../scripts/smoke/shipped-image/README.md) against the built image, with the unit
made the way the Docker driver makes one.

### The account and the volumes

- `agent`, uid and gid 1000, home `/home/agent`, shell `/bin/bash`; the working directory is `/workspace`.
- Nothing the image needs is under `/home` or `/workspace`: the home directory is a volume that outlives the
  image, so a program installed there would be hidden by an older home or left behind by an update. The
  browser, Node.js and the command line are under `/opt` and `/usr/local`; what they keep as state (the
  command line's sessions, a browser's own temporary profile) goes under `~/.cache` and `/tmp`.
- `/etc` and everything else outside the two volumes is gone when an environment is recreated or its image
  is updated, `sudo`'s installs included: what must last belongs in a template.

### Root without a password

- `/etc/sudoers.d/90-agent` holds `agent ALL=(ALL:ALL) NOPASSWD: ALL`, and `visudo --check` passes at build.
  Only that account has it; root has no password to give.
- It works in a unit as Docker makes one (default capabilities and seccomp profile, no `no_new_privileges`).
  Where the template sets `no_new_privileges` it is taken away, whatever the image: `sudo` then says `sudo: The
  "no new privileges" flag is set, which prevents sudo from running as root.` and exits 1, and everything else in
  the image still runs for the user.
- `sudo` resets the environment (as Debian's default does), so root works in `/root` and a tool run as root does
  not leave root-owned files in the user's home; it keeps the three variables below.
- The package lists are not in the image: `sudo apt-get update` comes before `sudo apt-get install`.

### Variables

Set in the image's environment (so in every process of a unit that is not given another value), kept by `sudo`
(`env_keep`), and set again for login shells that were not given them (`/etc/profile.d/orch-browser.sh`, which
leaves a value that is already there alone: `sudo -i`, `env -i bash -l`):

| Variable | Value | Why |
|----------|-------|-----|
| `PLAYWRIGHT_BROWSERS_PATH` | `/opt/ms-playwright` | Where the command line looks for browsers; without it, in `~/.cache/ms-playwright`, which is the home volume's and holds nothing from this image |
| `PLAYWRIGHT_MCP_BROWSER` | `chromium` | Which browser the command line starts when it is not told. Without it, Google Chrome (`/opt/google/chrome/chrome`), which the image does not hold: `Chromium distribution 'chrome' is not found at /opt/google/chrome/chrome`. With it, Chrome for Testing from the registry above |
| `NO_UPDATE_NOTIFIER` | `1` | The command line looks up its newest version on npm once a day unless told not to: a request from every unit to a server nobody chose, for a notice about a version the image does not move to |

Why a variable and not a configuration file: the command line reads `.playwright/cli.config.json` of the current
directory, and `~/.playwright/cli.config.json` of the home (and a file named by `PLAYWRIGHT_MCP_CONFIG`). The
first is per directory, the second is on the home volume, and neither is the image's; a variable reaches every
user and every directory. The order of what it reads is: the home's file, the directory's file (or the named one),
the `PLAYWRIGHT_MCP_*` variables, the command line's own options; so a person's or a project's own choice still wins.

### The browser

- **Program.** `/opt/ms-playwright/chromium-<revision>/chrome-linux64/chrome` (aarch64: `chrome-linux-arm64`);
  find it with the glob `$PLAYWRIGHT_BROWSERS_PATH/chromium-*/chrome-linux*/chrome`. There is one, and none
  under the names `chromium` or `google-chrome` on the path. `chrome --version` prints
  `Google Chrome for Testing 155.0.8059.12 ` (with a space at the end). It is Chrome for Testing, the full browser:
  `--headless` is that browser without a window (it calls itself `HeadlessChrome/155.0.0.0` in its user agent
  and keeps the lock on its profile); the separate headless shell that the command line would start for a browser
  it was not told the channel of is not installed. `ffmpeg` (for the command line's video recording) is beside it.
- **Command line.** `playwright-cli --version` prints `0.1.22`. `playwright-cli open <url>` starts a headless
  browser of its own on an in-memory profile with nothing configured; `playwright-cli attach --cdp=<endpoint>`
  drives a browser somebody else started, and `detach` leaves it running. The browser that `open` starts is
  started by Playwright with its own arguments, among them `--headless`, `--no-sandbox`, `--disable-dev-shm-usage`,
  a temporary profile under `/tmp` and `--remote-debugging-pipe`: it has no address for driving, only a pipe that
  the command line holds. Added to the command line's configuration, `launchOptions.args:
  ["--remote-debugging-port=0"]` would give it one; the image does not.
- **An address for driving.** Started with `--user-data-dir=<new directory> --remote-debugging-port=0` the browser
  picks a port, answers at `http://127.0.0.1:<port>/json/version` (`Browser` is `Chrome/155.0.8059.12`) and writes
  `<profile>/DevToolsActivePort`: two lines, the port, then the path of its socket
  (`/devtools/browser/<uuid>`), with no line break after the second. It also prints
  `DevTools listening on ws://127.0.0.1:<port>/devtools/browser/<uuid>`. It answered within a second here, on a
  machine with a load of twenty. Asked to quit with `Browser.close` over that socket, it
  ends within 10 s, and its profile is free again.
- **One profile, one running browser.** A second full browser on a profile in use ends at once with status 21 and
  `Failed to create a ProcessSingleton for your profile directory. This means that running multiple instances
  would start multiple browser processes rather than opening a new window in the existing process. Aborting now to
  avoid profile corruption.` The first browser is not disturbed, and the profile is free when it has ended.
- **Its sandbox, in a unit as Docker makes it.** There is none: the browser's sandbox needs the kernel's user
  namespaces, which Docker's default seccomp profile refuses (and `no_new_privileges` does not change that). Run as
  `agent` without `--no-sandbox` the browser aborts after about two seconds (status 134, SIGABRT) and prints
  `FATAL:content/browser/zygote_host/zygote_host_impl_linux.cc:130] No usable sandbox! If you are running on
  Ubuntu 23.10+ or another Linux distro that has disabled unprivileged user namespaces with AppArmor, see ...
  If you want to live dangerously and need an immediate workaround, you can try using --no-sandbox.` followed by
  a stack trace; with `--no-sandbox` it starts. Run as root it refuses without the flag whatever the container
  allows (`Running as root without --no-sandbox is not supported`). A container that allows user namespaces gives
  the browser its sandbox: `--security-opt seccomp=unconfined` and `--cap-add SYS_ADMIN` were both tried, and the
  browser then starts without the flag. Nothing in the image stands in its way; it is the unit's to give.
- **What a start that fails leaves.** Orch's Docker units start with a core-file soft limit of 0 and an
  unlimited hard limit. An aborted browser therefore leaves no core file in its working directory by default.
  A debugging shell may raise its own soft limit with `ulimit -c unlimited`; the host's core pattern then
  decides where a dump goes. The real-unit core-limit test covers the default and the shell's explicit change.
- **Noise on its standard error**, harmless: `dbus/bus.cc ... Failed to connect to the bus` (no session bus
  runs), `GLib-GIO-CRITICAL ... g_settings_schema_source_lookup`, and the crashpad lines about `cpufreq`.
  When a start fails, the line to look for is the one with `FATAL` or `ERROR:content/` or
  `ERROR:chrome/app/chrome_main_delegate.cc`.
- **A window.** On a display (below) the browser opens a window named `<page title> - Google Chrome for Testing`
  (window class `Chromium-browser`, its instance name holds the profile's directory); it writes
  `DevToolsActivePort` and answers at its address as without a window. Chrome for Testing shows a note in the
  window, "Chrome for Testing ... is only for automated testing", and a "Sign in to Chromium?" button; nothing in
  the image switches them off.
- **Shared memory.** A unit's `/dev/shm` is 64 MB (Docker's default, which the driver does not change). The browser
  uses it for what its processes share; a page that needs more makes a tab crash, and `--disable-dev-shm-usage`
  moves that use to `/tmp` (Playwright passes it to the browsers it starts; one started by hand has to be given
  it). A page that does need more was not tried.

### Fonts

Debian's Noto CJK (one family with the variant of each language: `Noto Sans CJK SC`, `TC`, `JP`, `KR` and
`HK`, sans and serif, proportional and mono), Noto Color Emoji, Liberation (the metrics of Arial, Times and
Courier), DejaVu, and Unifont as the last resort for any script of the basic plane that nothing else draws
(Thai, Devanagari and others are shown, not boxed). A page in Chinese, Japanese or Korean, marked with its
language, is drawn with the font of that language, which `CSS.getPlatformFontsForNode` reports and the scenario
checks (`zh-CN` with SC, `zh-TW` with TC, `ja` with JP, `ko` with KR, emoji with Noto Color Emoji). A page without
a character set is the page's fault: a `data:` URL needs `;charset=utf-8`.

### The display

- **Server.** `/usr/bin/Xvnc` (a link, through `/etc/alternatives/Xvnc`, to `/usr/bin/Xtigervnc`). `Xvnc -version`
  prints to the standard *error* a blank line and then `Xvnc TigerVNC 1.12.0 - built 2023-01-06 16:01` and exits 0;
  `--version` is refused.
- **Started as a display is started** (on a free X display `:N`, a free port `P`):
  `Xvnc :N -geometry 1280x800 -depth 24 -rfbport P -localhost -AlwaysShared -SecurityTypes None -SendCutText=0`.
  Each of these was checked: it listens on 127.0.0.1 only (and its own address in the unit refuses a connection),
  speaks the protocol's version 3.8 and offers no way of asking who is there but none, shows a 1280x800 screen
  24 bits deep, keeps a first viewer when a second asks for the screen to itself (a server without `-AlwaysShared`
  drops it), and leaves neither its lock nor its socket when it is stopped. `xdpyinfo` answers on `DISPLAY=:N`.
- **`/tmp/.X11-unix`** exists, owned by root with mode 1777, so that a server run by the agent does not warn that
  it cannot make it (and then make it itself, owned by the agent). A unit that mounts something over `/tmp`
  loses it, and the warning is the only harm.
- **Window manager.** `/usr/bin/openbox` (`openbox --version` prints `Openbox 3.6.1` first). Started on the display
  it is its window manager (`_NET_SUPPORTING_WM_CHECK` names a window called `Openbox`). It prints `Openbox-Message:
  Unable to find a valid menu file "/var/lib/openbox/debian-menu.xml"`: the menu package is not installed, which
  costs the right-click menu its Debian entries and nothing else. There is no desktop, no session, no terminal
  emulator and no wallpaper: the root window is black.
- **What a program needs to open a window there**: `DISPLAY`, the X libraries (the browser's own libraries pull the
  usual ones), `xauth`, the keyboard data (`xkb-data`, `x11-xkb-utils`), the core fonts (`xfonts-base`, which a bare
  X server cannot start without and which Debian only recommends) and the fonts above, and `x11-utils` and
  `x11-xserver-utils` for looking at a display (`xdpyinfo`, `xwininfo`, `xprop`, `xrandr`, `xset`). The server brings
  Mesa's software OpenGL with it.

## Building

```sh
docker build --tag orch-env-base:0.1.0 --build-arg VERSION=0.1.0 images/orch-env-base
cargo xtask image --version 0.1.0 --fresh      # the same, without Docker's cache, and the labels checked
```

`cargo xtask package` builds it as `orch-env-base:<version>`, which is the tag the API seeds the Default template
with, so a node on the same Docker daemon finds it without pulling, and records its content address in the
release's manifest. A build from a clean cache took four minutes on a machine with a load of twenty-five (the
downloads are Node.js, the npm packages, 190 MB of browser and the Debian packages), and needs the network.

The recipe builds for the processor Docker runs on. Only x86_64 has been built and run; the pins for aarch64 are
the same (the Node.js archive of that processor has its hash in the recipe, and the browser, whose directory is then
`chrome-linux-arm64`, is the one Playwright's registry names for it), and nothing of it has been tried.

## Moving a pin

Read the vendor's own page, change the arguments at the head of the Dockerfile together, and build:

1. Node.js: pick a line that is maintained (<https://nodejs.org/en/about/previous-releases>), and take the
   SHA-256 of `node-v<version>-linux-x64.tar.xz` and of `-linux-arm64.tar.xz` from
   `https://nodejs.org/dist/v<version>/SHASUMS256.txt` (the vendor's list, which it also signs).
2. The command line: `npm view @playwright/cli@<version> dependencies` names the one build of Playwright it uses
   (`PLAYWRIGHT_VERSION`); that build's `browsers.json` (in its `playwright-core` package) gives the browser's
   `revision` and `browserVersion`.
3. TigerVNC and openbox: the upstream part of the version Debian carries (`apt-cache policy tigervnc-standalone-server
   openbox` in the base image), without the revision after it, which is allowed to move.
4. Write each version into this file, run `cargo xtask image --fresh --version try`, then
   `scripts/smoke/shipped-image/run.sh`.

## Third-party software

The component terms are listed in [NOTICE.md](NOTICE.md), installed at
`/usr/share/doc/orch-env-base/NOTICE.md`. The image's licence label is
`LicenseRef-Orch-Env-Base`, referring to that notice; the recipe's own Apache-2.0
licence has a separate label. The browser's executable has its maker's terms,
and the remaining packages keep their own licence and copyright files.

`LANG=C.UTF-8` is set for every process that inherits the image's environment,
so ordinary shells display Unicode file names and text correctly.
