"""Wrapper functions that route Python entry points to the Rust CLI."""

from __future__ import annotations

import sys
from typing import Iterable, Sequence

from . import _wtg


def run(argv: Iterable[str] | None = None) -> int:
    """Execute the CLI using the provided argv sequence.

    When *argv* is ``None`` the current ``sys.argv`` is forwarded, which mirrors
    invoking the tool as a standalone executable.
    """

    args: list[str]
    if argv is None:
        args = list(sys.argv)
    elif isinstance(argv, Sequence):
        args = list(argv)
    else:
        args = list(argv)

    if not args:
        args = ["wtg"]

    return _wtg.run_cli([str(arg) for arg in args])


def main() -> int:
    """Entry point used by ``python -m wtg_cli`` and console scripts."""

    return run()
