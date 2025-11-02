"""Python helpers for the What The Git CLI package."""

from importlib import metadata as _metadata

from .cli import main, run

try:
    __version__ = _metadata.version("wtg-cli")
except (
    _metadata.PackageNotFoundError
):  # pragma: no cover - fallback for editable installs
    __version__ = "0.0.0"

__all__ = ["main", "run", "__version__"]
