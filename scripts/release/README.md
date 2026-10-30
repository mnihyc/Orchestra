# Stock release images

`python3 -I -B scripts/release/stock-images.py` queries the current Docker registry index for each stock
tag and prints the proposed references. It changes no file. Add `--write` to refresh the discovered
recipe, workflow and script image inputs after reviewing the indexes. Docker with the Buildx plugin is
needed for these lookups. Each pin names the whole image index, so it covers every platform in that
index with one value.

`python3 -I -B scripts/release/stock-images.py --check` checks the selected references and Action commits
offline throughout the tracked tree, including newly added input files. Local operator helpers explicitly
use `--pull=never` to prevent registry fallback. CI runs this gate, and `python3 -I -B scripts/release/test_stock_images.py`
exercises recipes, workflow containers, script selectors and local helpers. Packaging records stock index digests in `version.json` and verifies the
Compose selections against that manifest when checking a release.
