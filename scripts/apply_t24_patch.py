#!/usr/bin/env python3
"""Apply the bundled T24 OTIS Twin patch to a Leviathan checkout.

Keep t24-otis-twin.patch beside this script. The patch targets the Wave 5
baseline (e7b0bfb). Use --check to verify applicability. Applying updates
both the working tree and Git index so the result can be reviewed with
git diff --cached.
"""

import argparse
import subprocess
import sys
from pathlib import Path


def git(repo: Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", "-C", str(repo), *args],
        capture_output=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd(), help="Leviathan checkout")
    parser.add_argument("--check", action="store_true", help="Check applicability without writing")
    args = parser.parse_args()
    patch = Path(__file__).resolve().with_name("t24-otis-twin.patch")
    if not patch.is_file():
        parser.error(f"Missing bundled patch {patch}")

    try:
        root = git(args.repo.resolve(), "rev-parse", "--show-toplevel")
        if root.returncode:
            print(root.stderr.rstrip(), file=sys.stderr)
            return 1
        repo = Path(root.stdout.rstrip("\r\n"))
        check = git(repo, "apply", "--index", "--check", "--", str(patch))
        if check.returncode:
            reverse = git(repo, "apply", "--index", "--check", "--reverse", "--", str(patch))
            if reverse.returncode:
                print(f"T24 patch could not be applied.\n{check.stderr.rstrip()}", file=sys.stderr)
                return 1
            print("T24 patch is already applied.")
            return 0
        if args.check:
            print("T24 patch applies cleanly.")
            return 0

        applied = git(repo, "apply", "--index", "--", str(patch))
        if applied.returncode:
            print(applied.stderr.rstrip(), file=sys.stderr)
            return 1
    except OSError as error:
        print(error, file=sys.stderr)
        return 1

    print("Applied and staged the T24 patch. Review with git diff --cached.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
