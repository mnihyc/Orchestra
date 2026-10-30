#!/usr/bin/env python3
"""Package a native connector and, on Linux, its matching node with size and hash manifests."""

import argparse
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    subprocess.run(["rustup", "show"], check=True)
    subprocess.run(["cargo", "xtask", "package", "--out", str(args.out.resolve()), "--native-connector", args.target], check=True)


if __name__ == "__main__":
    main()
