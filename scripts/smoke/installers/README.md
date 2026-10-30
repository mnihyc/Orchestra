# installers

The command a person pastes from "Add machine", on Linux, against a running system behind an HTTPS
front whose certificate the system's own authority signed. This native scenario exercises HTTPS;
the offline address fixtures also cover the installer's loopback-only HTTP exception.

| Promise | How it is shown |
|---------|-----------------|
| The installer the system serves fetches the connector from the system and checks it | `curl …/install.sh \| sh -s --` in a fresh home; it says the SHA-256 is as `version.json` lists it |
| It places the connector for the person and enrolls the machine | `~/.local/bin/orch-connector`, the identity in the data directory, and the machine in `GET /machines` |
| The token is never printed | The installer's output does not hold it |
| A retained identity accepts only its bound replacement token | A new-machine token is refused with Enroll again; its id, program and local settings stay. The bound replacement then succeeds in that same directory |
| A connector that does not match the manifest is refused, and nothing is left | One byte added to the served file: refused, no connector and no identity in that home |
| Help needs no token, root, platform probe, download or installation | `help.sh` sources both shell help branches in English and Chinese with platform/network/file/service commands forbidden; `help.ps1` exercises both Windows help aliases with installation operations forbidden |
| HTTP system addresses are accepted only for literal loopback hosts | `addresses.tsv` is shared by `addresses.sh` (both shell installers, curl/wget, English/Chinese) and `addresses.ps1` (Windows). It covers 127/8 boundaries, localhost, compressed/expanded IPv6 loopback, ports, remote hosts, lookalikes, userinfo, encoded hosts, malformed addresses and CR/LF. Accepted cases reach stubbed manifest/download calls; refusals happen before them and name HTTPS plus the permitted loopback hosts |
| Redirects cannot widen that HTTP exception | Curl only follows HTTPS redirects; wget and PowerShell disable redirects. The fixtures assert these transport arguments. The node's separate pinned gateway address still requires wss/https; HTTP applies to its system download address `--from` |

Native installation is not exercised for macOS or Windows; Windows help and parsing run when PowerShell
is available. Per-user services have no user manager in the scenario (`--no-service`). The node's
installation still needs a root Linux fixture: check the default service and `--no-service` paths there;
the latter must print the foreground command without requiring systemd or writing a unit. Both shell
scripts are also read by `sh -n`. These are proof obligations, not writer pass claims.

```sh
scripts/smoke/installers/run.sh
# Offline help regressions only; no release, stack or installed connector needed:
sh scripts/smoke/installers/help.sh
pwsh -NoProfile -File scripts/smoke/installers/help.ps1
sh scripts/smoke/installers/addresses.sh
pwsh -NoProfile -File scripts/smoke/installers/addresses.ps1
```

The address fixtures never run a real download, enroll, install, service or removal. The shell
fixture simulates the platform/root facts only up to the first stubbed download and forbids file
placement. The Windows fixture stubs both manifest and asset requests and restores its process-local
platform environment. These proofs do not replace native install/connection trials, including the
connector enrollment client's loopback HTTP transport and the enrollment dialog's eligibility supplier.
