#!/usr/bin/env python3
"""Extract only classes/module-info.class from a JMOD without unpacking it."""

from pathlib import Path
import sys
import zipfile


with zipfile.ZipFile(sys.argv[1]) as archive:
    Path(sys.argv[2]).write_bytes(archive.read("classes/module-info.class"))
