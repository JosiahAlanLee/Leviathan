#!/usr/bin/env python3
"""Apply the bundled T25 and T26 patches to the merged T24 checkout (ff0c9b8).

With T25 already present, use --t26-only. Use --check to preview either
diff before staging changes.
"""

import argparse
import sys
from pathlib import Path

# Preflight also works from the target checkout without writing a module cache.
sys.dont_write_bytecode = True
from apply_t24_patch import apply_patch


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd(), help="Leviathan checkout")
    parser.add_argument("--check", action="store_true", help="Check applicability without writing")
    parser.add_argument("--t26-only", action="store_true", help="Apply FS77 to a checkout with T25")
    args = parser.parse_args()
    name = "t26-fs77.patch" if args.t26_only else "t25-t26-fs77.patch"
    return apply_patch(args.repo, Path(__file__).resolve().with_name(name), args.check)


if __name__ == "__main__":
    raise SystemExit(main())
