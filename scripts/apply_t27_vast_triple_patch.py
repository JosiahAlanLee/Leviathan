#!/usr/bin/env python3
"""Apply the bundled K26 patch to a Leviathan checkout with T26 (e26167c)."""

import argparse
import sys
from pathlib import Path

sys.dont_write_bytecode = True
from apply_t24_patch import apply_patch


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd(), help="Leviathan checkout")
    parser.add_argument("--check", action="store_true", help="Check applicability without writing")
    args = parser.parse_args()
    patch = Path(__file__).resolve().with_name("t27-vast-triple.patch")
    return apply_patch(args.repo, patch, args.check)


if __name__ == "__main__":
    raise SystemExit(main())
