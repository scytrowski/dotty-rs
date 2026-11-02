#!/usr/bin/env python3
"""Compare the complete oracle receipt except the explicitly informational JAR digest."""
import json
import pathlib
import sys


def comparable(path: pathlib.Path) -> dict:
    data = json.loads(path.read_text())
    data["oracle"].pop("scalac_jar_sha256_informational", None)
    return data


generated, frozen = map(pathlib.Path, sys.argv[1:])
if comparable(generated) != comparable(frozen):
    raise SystemExit("oracle manifest differs from frozen evidence")
