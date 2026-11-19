# Common passwords

The release's own list of commonly used passwords, which every entrance that sets a password compares the whole
new password against (case folded, never a part of it). It is release data, read from the binary: nothing is
fetched when the server runs, and a list that does not match the figures below is a fault of the build, never an
empty list.

- `10k-most-common.txt`: `Passwords/Common-Credentials/10k-most-common.txt` of
  [SecLists](https://github.com/danielmiessler/SecLists) at commit `913b327317496d062bcc7cace524aaad8a693be2`,
  unchanged: 10,001 distinct nonempty lines, 73,026 bytes, SHA-256
  `68782d6a4a19a4768d5f15dd66bd534e7a33055cc755411e33f16d18c50fdcce`.
- `LICENSE`: SecLists' MIT licence, which comes with the data.

The code adds `password1234` beside the file (`sign_in/passwords.rs`): a value people use that this file lacks.
