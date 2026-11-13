#!/usr/bin/env python3
"""Apply the bundled T24 OTIS Twin patch to a Leviathan checkout.

Keep t24-otis-twin.patch beside this script. The patch targets the Wave 5
baseline (e7b0bfb). Use --check to verify applicability. Applying updates
both the working tree and Git index so the result can be reviewed with
git diff --cached.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path


def git(repo: Path, *args: str, stdin: bytes | None = None) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        ["git", "-C", str(repo), *args],
        input=stdin,
        capture_output=True,
        check=False,
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd(), help="Leviathan checkout")
    parser.add_argument("--check", action="store_true", help="Check applicability without writing")
    args = parser.parse_args()
    patch = Path(__file__).resolve().with_name("t24-otis-twin.patch")
    return apply_patch(args.repo, patch, args.check)


def apply_patch(repo: Path, patch: Path, check_only: bool) -> int:
    if not patch.is_file():
        print(f"Missing bundled patch {patch}", file=sys.stderr)
        return 1

    try:
        # Git may check the bundled text patch out with CRLF. Pass the
        # canonical bytes through stdin to avoid platform newline conversion.
        diff = patch.read_bytes().replace(b"\r\n", b"\n")
        root = git(repo.resolve(), "rev-parse", "--show-toplevel")
        if root.returncode:
            print(root.stderr.decode(errors="replace").rstrip(), file=sys.stderr)
            return 1
        repo = Path(os.fsdecode(root.stdout.rstrip(b"\r\n")))
        check = git(repo, "apply", "--index", "--check", "-", stdin=diff)
        if check.returncode:
            reverse = git(repo, "apply", "--index", "--check", "--reverse", "-", stdin=diff)
            if reverse.returncode:
                print(f"{patch.name} could not be applied.", file=sys.stderr)
                print(check.stderr.decode(errors="replace").rstrip(), file=sys.stderr)
                return 1
            print(f"{patch.name} is already applied.")
            return 0
        if check_only:
            print(f"{patch.name} applies cleanly.")
            return 0

        applied = git(repo, "apply", "--index", "-", stdin=diff)
        if applied.returncode:
            print(applied.stderr.decode(errors="replace").rstrip(), file=sys.stderr)
            return 1
    except OSError as error:
        print(error, file=sys.stderr)
        return 1

    print(f"Applied and staged {patch.name}. Review with git diff --cached.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
