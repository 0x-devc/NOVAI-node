"""Put the meta-agent directory on sys.path so tests can import the meta package."""

from __future__ import annotations

import sys
from pathlib import Path

META_DIR = Path(__file__).resolve().parent.parent
if str(META_DIR) not in sys.path:
    sys.path.insert(0, str(META_DIR))
