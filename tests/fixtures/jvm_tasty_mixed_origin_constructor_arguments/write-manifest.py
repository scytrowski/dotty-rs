#!/usr/bin/env python3
import hashlib
import json
import pathlib
import struct
import subprocess
import sys

scala_root, java_home, fixture, output = map(pathlib.Path, sys.argv[1:])
fixture = fixture.resolve()
output = output.resolve()


def sha(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def class_info(path: pathlib.Path) -> tuple[int, bytes]:
    data = path.read_bytes()
    at = 0

    def u1() -> int:
        nonlocal at
        result = data[at]
        at += 1
        return result

    def u2() -> int:
        nonlocal at
        result = struct.unpack_from(">H", data, at)[0]
        at += 2
        return result

    def u4() -> int:
        nonlocal at
        result = struct.unpack_from(">I", data, at)[0]
        at += 4
        return result

    if u4() != 0xCAFEBABE:
        raise ValueError(f"not a classfile: {path}")
    u2()
    major = u2()
    count = u2()
    utf8: dict[int, str] = {}
    index = 1
    while index < count:
        tag = u1()
        if tag == 1:
            size = u2()
            utf8[index] = data[at : at + size].decode("utf-8")
            at += size
        elif tag in (3, 4):
            at += 4
        elif tag in (5, 6):
            at += 8
            index += 1
        elif tag in (7, 8, 16, 19, 20):
            at += 2
        elif tag in (9, 10, 11, 12, 17, 18):
            at += 4
        elif tag == 15:
            at += 3
        else:
            raise ValueError(f"unknown constant-pool tag {tag} in {path}")
        index += 1
    at += 6
    interfaces = u2()
    at += interfaces * 2
    for _ in range(2):
        members = u2()
        for _ in range(members):
            at += 6
            attributes = u2()
            for _ in range(attributes):
                at += 2
                length = u4()
                at += length
    attributes = u2()
    tasty_uuid = None
    for _ in range(attributes):
        name = utf8[u2()]
        length = u4()
        payload = data[at : at + length]
        at += length
        if name == "TASTY":
            if len(payload) != 16:
                raise ValueError(f"unexpected TASTY attribute length in {path}")
            tasty_uuid = payload
    if tasty_uuid is None:
        raise ValueError(f"missing TASTY attribute in {path}")
    return major, tasty_uuid


def tasty_info(data: bytes) -> tuple[str, bytes]:
    offset = 0

    def nat() -> int:
        nonlocal offset
        value = 0
        while True:
            byte = data[offset]
            offset += 1
            value = (value << 7) | (byte & 0x7F)
            if byte & 0x80:
                return value

    if data[:4] != bytes.fromhex("5ca1ab1f"):
        raise ValueError("invalid TASTy magic")
    offset = 4
    major, minor, experimental = nat(), nat(), nat()
    tooling_length = nat()
    offset += tooling_length
    uuid = data[offset : offset + 16]
    if len(uuid) != 16 or (major, minor, experimental) != (28, 9, 0):
        raise ValueError(f"unexpected TASTy header: {major}.{minor}.{experimental}")
    return f"{major}.{minor}.{experimental}", uuid


revision = subprocess.check_output(["git", "-C", str(scala_root), "rev-parse", "HEAD"], text=True).strip()
compiler_jar = scala_root / "dist/target/universal/stage/maven2/org/scala-lang/scala3-compiler_3/3.9.0-bin-SNAPSHOT/scala3-compiler_3-3.9.0-bin-SNAPSHOT.jar"
java = java_home / "bin/java"
javap = java_home / "bin/javap"
java_version = subprocess.run([str(java), "-version"], capture_output=True, text=True)
javap_version = subprocess.check_output([str(javap), "-version"], text=True).strip()
compiler_hash = sha(compiler_jar)
java_hash = sha(java)
if compiler_hash != "b56626390028bd06347e3549cf271a03ca7d2e7be491ceff630ccee94e439f4f":
    raise ValueError(f"unexpected Scala compiler JAR hash: {compiler_hash}")
if java_hash != "9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155":
    raise ValueError(f"unexpected JDK java binary hash: {java_hash}")
if "21.0.13" not in (java_version.stdout + java_version.stderr):
    raise ValueError("expected Temurin JDK 21.0.13")
manifest = {
    "schema_version": 1,
    "purpose": "Scala 3.9-produced oracle only; this fixture does not assert Rust producer support or Scala/Rust interoperability.",
    "baseline": "origin/main at 7943e8d495815ceef2b7e4a53baee7f6fcac949b",
    "oracle": {
        "scala_revision": revision,
        "scala_expected_revision": "777528f19a58e794c9954a42f433373472ec57f8",
        "scalac_version": "3.9.0-bin-SNAPSHOT-git-777528f",
        "scalac_jar_sha256": compiler_hash,
        "jdk_version": (java_version.stdout + java_version.stderr).strip().splitlines()[0],
        "java_sha256": java_hash,
        "javap_version": javap_version,
        "classfile_major": 65,
        "release": 21,
        "tasty_version": "28.9.0",
        "producer_artifact_counts_per_order": {"class": 3, "tasty": 3},
        "outputs": "Both order-specific producer directories are compiled from empty output; each consumer is compiled separately against its matching Scala producer directory and Scala runtime.",
    },
    "orders": {},
}
if revision != manifest["oracle"]["scala_expected_revision"]:
    raise ValueError(f"wrong Scala checkout revision: {revision}")
for order, method in (("wrap-and-create", "wrapAndCreate"), ("create-and-wrap", "createAndWrap")):
    source = fixture / order / "PairFactory.scala"
    artifacts = []
    for name in ("Counter", "Pair", "PairFactory"):
        cls = output / "scala39" / order / "example" / f"{name}.class"
        tasty = output / "scala39" / order / "example" / f"{name}.tasty"
        major, linked_uuid = class_info(cls)
        tasty_data = tasty.read_bytes()
        tasty_version, tasty_uuid = tasty_info(tasty_data)
        if tasty_version != "28.9.0" or linked_uuid != tasty_uuid:
            raise ValueError(f"TASTy header/link mismatch: {name} {order}")
        artifacts.extend(
            [
                {"path": str(cls.relative_to(output)), "size_bytes": cls.stat().st_size, "sha256": sha(cls), "classfile_major": major, "linked_tasty_uuid": linked_uuid.hex()},
                {"path": str(tasty.relative_to(output)), "size_bytes": tasty.stat().st_size, "sha256": sha(tasty), "tasty_version": tasty_version, "uuid": tasty_uuid.hex()},
            ]
        )
    snapshots = {}
    for name in ("Counter", "Pair", "PairFactory"):
        for suffix in ("javap", "tasty"):
            path = output / "oracle" / order / f"{name}.{suffix}.txt"
            snapshots[path.name] = {"sha256": sha(path)}
    manifest["orders"][order] = {
        "method": method,
        "source": {"path": str(source.relative_to(fixture)), "sha256": sha(source)},
        "consumer": {
            "path": "Consumer.scala" if order == "wrap-and-create" else "ConsumerReversed.scala",
            "sha256": sha(
                fixture / ("Consumer.scala" if order == "wrap-and-create" else "ConsumerReversed.scala")
            ),
            "compiled_against": "only the matching Scala-produced producer output and Scala 3.9 runtime",
            "result": "passed: formal object identity, fresh nested allocation, repeated factory calls, signed seed, independent mutation",
        },
        "artifacts": artifacts,
        "snapshots": snapshots,
    }
(output / "oracle.json").write_text(json.dumps(manifest, indent=2) + "\n")
