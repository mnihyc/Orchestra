# shipped-image scenario

The image every default environment is made from, looked at from inside a unit made the way the Docker driver
makes one (an init process, the account the template names, the home directory and the workspace on volumes),
with no server, no release and no network but the one a unit has. It shows what
[the image's README](../../../images/orch-env-base/README.md) promises, and prints for each promise `PASS` or
`FAIL`.

1. **The image says what it holds.** Every label with a version (Node.js, the command line, the Playwright it
   depends on, the browser and its revision, TigerVNC, openbox) equals what the program inside prints, and what the
   image held before (a shell, git, a search, a compiler, Python, curl) is still there. Where the image that
   `ORCH_SMOKE_IMAGE` names (`orch-env-base:smoke-shipped-image` by default) is not there, it is first built from
   its recipe by `docker build` alone, as the scenario's first check.
2. **Root without a password.** In a unit as the Default template makes it, `sudo` asks for nothing and what it
   runs is root; a file it writes in `/etc` is root's and the user cannot write there without it; root works in
   `/root` and keeps the three variables that find the browser; a package is installed through `apt` and the user
   runs it (skipped, and said so, where the unit has no route to the Debian mirrors). In a unit whose template sets
   `no_new_privileges`, `sudo` does not run, says why, and the rest of the image still runs for the user.
3. **The browser and its command line.** Node.js, `npm` and `playwright-cli` are outside the home and the workspace,
   and the three variables reach the shell, a login shell started with an empty environment and root. With nothing
   configured `playwright-cli open` starts the image's browser, which draws a page in Chinese (simplified and
   traditional), Japanese, Korean and with emoji in the fonts of those languages (the fonts it used are asked of
   the browser itself, and a picture is kept); without the variable it asks for Google Chrome and finds none.
   The full browser is then started as a host starts one (a new profile, `--remote-debugging-port=0`, no window):
   with its sandbox asked for first, and, where the unit cannot give one, once more without, with what it said
   printed (under Docker's defaults it cannot: it ends with status 134 and `No usable sandbox!`); it answers at its
   address, which `DevToolsActivePort` names; `playwright-cli attach --cdp` drives it and `detach` leaves it
   running; a second browser on the same profile is refused (status 21, `ProcessSingleton`) and the first is not
   disturbed; asked to quit through its protocol it ends and the profile is free again.
4. **The sandbox where the container allows it.** In a container whose seccomp profile is off, the browser starts
   without `--no-sandbox`, which it only does if it has its own sandbox.
5. **A display.** `Xvnc` is started on the first free X display with the options a display is started with
   (`-geometry 1280x800 -depth 24 -rfbport <free port> -localhost -AlwaysShared -SecurityTypes None
   -SendCutText=0`): it listens on the loopback address only, a client speaks the protocol's version 3.8 and is
   offered no way of asking who is there but none, the screen is 1280x800 at depth 24, a second viewer that asks for
   the screen to itself does not take it from the first (and a server started without `-AlwaysShared` does drop it:
   the control that shows the check can tell). `openbox` runs on it as its window manager, a browser with a window
   shows there (the X server lists a window of the title the page has), a viewer sees the screen change (a picture
   is kept), and a stopped server leaves neither its lock nor its socket.

```sh
scripts/smoke/shipped-image/run.sh                                      # builds the image from its recipe, as orch-env-base:smoke-shipped-image
ORCH_SMOKE_IMAGE=orch-env-base:0.1.0 scripts/smoke/shipped-image/run.sh # takes the image of that name if there is one, builds it if not
ORCH_SMOKE_NO_CACHE=1 scripts/smoke/shipped-image/run.sh                # builds it without Docker's cache
```

It needs Docker and the network (the image's build downloads Node.js, the npm packages, the browser and the Debian
packages; the unit installs one small package). It needs no release, publishes no port and takes about a minute on
a machine of ordinary load once the image is there. At the end it removes its three containers (the unit, the one
with `no_new_privileges` and the one without a seccomp profile), the two volumes of the unit, the image if it built
it, and its directory, by exact names; `ORCH_SMOKE_KEEP=1` leaves them, and the pictures it took are in
`shipped-image/evidence` below `ORCH_SMOKE_DIR`.
