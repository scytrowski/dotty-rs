#!/usr/bin/env python3
"""Compile source-import oracle fixtures with the pinned Scala 3.9 compiler."""

import os
import re
import subprocess
import tempfile
from pathlib import Path


def compile_sources(
    java_home: Path, classpath: str, output: Path, sources: list[Path]
) -> subprocess.CompletedProcess[str]:
    output.mkdir(parents=True, exist_ok=True)
    java_tmp = output.parent / "java-tmp"
    java_tmp.mkdir(parents=True, exist_ok=True)
    compiler_cp = os.pathsep.join([item for item in classpath.split(os.pathsep) if item])
    command = [
        str(java_home / "bin" / "java"),
        f"-Djava.io.tmpdir={java_tmp}",
        "-cp",
        compiler_cp,
        "dotty.tools.dotc.Main",
        "-classpath",
        compiler_cp,
        "-d",
        str(output),
        *(str(source) for source in sources),
    ]
    return subprocess.run(command, text=True, capture_output=True, check=False)


def main() -> int:
    java_home = Path(os.environ["JAVA_HOME"])
    classpath = os.environ["SCALA39_CLASSPATH"]
    with tempfile.TemporaryDirectory(prefix="scala39-import-oracle-") as temporary:
        root = Path(temporary)
        source = root / "source"
        output = root / "classes"
        source.mkdir()
        output.mkdir()
        fixtures = {
            "Defs.scala": "package a.b\nclass C\nobject Obj { val value = 1 }\n",
            "StackedCore.scala": "package a.core\nobject Obj { val value = 1 }\n",
            "Deep.scala": "package middle.deep.third\nclass Leaf\n",
            "RootLocal.scala": "package b\nobject Local { class Root }\n",
            "NestedLocal.scala": "package a.b\nobject Local { class Nested }\n",
            "RelativeLocal.scala": "package a.consumer.b\nobject Local { class Relative }\n",
            "RootCandidate.scala": "package candidate\nobject Local { class Target }\n",
            "NameCollisionPackage.scala": "package collision.Target\nclass FromPackage\n",
            "ImportClass.scala": "package fixture\nimport a.b.C\nobject ImportClass { val value: C = new C }\n",
            "ImportObject.scala": "package fixture\nimport a.b.Obj\nobject ImportObject { val value = Obj.value }\n",
            "ImportObjectMember.scala": "package fixture\nimport a.b.Obj.value\nobject ImportObjectMember { val result = value }\n",
            "ImportRename.scala": "package fixture\nimport a.b.{C => Alias}\nobject ImportRename { val value: Alias = new Alias }\n",
            "ImportRoot.scala": "package fixture\nimport _root_.a.b.C\nobject ImportRoot { val value: C = new C }\n",
            "ImportIntermediate.scala": "package fixture\nimport _root_.middle.deep.third.Leaf\nobject ImportIntermediate { val value: Leaf = new Leaf }\n",
            "ImportSourceRelative.scala": "package a.consumer\nimport b.Local.Relative\nobject ImportSourceRelative { val value: Relative = new Relative }\n",
            "ImportImportedQualifierShadowsPackage.scala": "package a.issue1174.consumer.b { object Local { class PackageRelative } }; package other { object b { object Local { class Relative } } }; package a.issue1174.consumer { import other.b; import b.Local.Relative; object ImportImportedQualifierShadowsPackage { val value: Relative = new Relative } }\n",
            "ImportStackedPackage.scala": "package a\npackage ast\nimport core.Obj\nobject ImportStackedPackage { val value = Obj.value }\n",
            "ImportRootFallback.scala": "package z.consumer\nimport b.Local.Root\nobject ImportRootFallback { val value: Root = new Root }\n",
            "ImportPackage.scala": "package fixture\nimport collision.Target.FromPackage\nobject ImportPackage { val value: FromPackage = new FromPackage }\n",
        }
        files = []
        for name, contents in fixtures.items():
            path = source / name
            path.write_text(contents, encoding="utf-8")
            files.append(path)
        compiled = compile_sources(java_home, classpath, output, files)
        if compiled.returncode:
            raise SystemExit(
                "pinned Scala 3.9 positive import fixtures failed:\n"
                + compiled.stdout
                + compiled.stderr
            )

        dotted_package = source / "ImportDottedPackage.scala"
        dotted_package.write_text(
            "package a.ast\nimport core.Obj\n"
            "object ImportDottedPackage { val value = Obj.value }\n",
            encoding="utf-8",
        )
        dotted_lookup = compile_sources(
            java_home,
            classpath + os.pathsep + str(output),
            root / "dotted-package-scope",
            [dotted_package],
        )
        dotted_diagnostic = dotted_lookup.stdout + dotted_lookup.stderr
        if dotted_lookup.returncode == 0 or "core" not in dotted_diagnostic:
            raise SystemExit(
                "Scala 3.9 treated a dotted package parent as a lexical import scope"
            )

        lexical_term = source / "ImportLexicalTermAndRoot.scala"
        lexical_term.write_text(
            "package a.consumer\n"
            "object candidate { object Local { class Other } }\n"
            "import candidate.Local.Other\n"
            "import _root_.candidate.Local.Target\n"
            "object ImportLexicalTermAndRoot { val local: Other = new Other; "
            "val rooted: Target = new Target }\n",
            encoding="utf-8",
        )
        lexical_term_result = compile_sources(
            java_home,
            classpath + os.pathsep + str(output),
            root / "lexical-term-and-root",
            [lexical_term],
        )
        if lexical_term_result.returncode:
            raise SystemExit(
                "Scala 3.9 lexical term and explicit root fixtures failed:\n"
                + lexical_term_result.stdout
                + lexical_term_result.stderr
            )

        local_term = source / "ImportLexicalTermNoRootFallback.scala"
        local_term.write_text(
            "package a.consumer\n"
            "object candidate { object Local { class Other } }\n"
            "import candidate.Local.Target\n"
            "object ImportLexicalTermNoRootFallback { val value: Target = new Target }\n",
            encoding="utf-8",
        )
        local_term_lookup = compile_sources(
            java_home,
            classpath + os.pathsep + str(output),
            root / "lexical-term-no-root-fallback",
            [local_term],
        )
        local_term_diagnostic = local_term_lookup.stdout + local_term_lookup.stderr
        if local_term_lookup.returncode == 0 or "Target is not a member" not in local_term_diagnostic:
            raise SystemExit(
                "Scala 3.9 fell back to a root package after selecting a lexical term qualifier"
            )
        if (output / "middle" / "deep" / "third" / "Leaf.class").is_file() is False:
            raise SystemExit("intermediate-only package fixture did not emit its leaf class")
        if list((output / "middle").glob("*.class")) or list(
            (output / "middle" / "deep").glob("*.class")
        ):
            raise SystemExit("package intermediates unexpectedly emitted direct classes")

        absent = source / "ImportMissingRoot.scala"
        absent.write_text(
            "package fixture\nimport _root_.missing.pkg.Absent\nobject ImportMissingRoot { val value = Absent }\n",
            encoding="utf-8",
        )
        missing_root = compile_sources(java_home, classpath + os.pathsep + str(output), root / "missing-root", [absent])
        if missing_root.returncode == 0 or "missing" not in (missing_root.stdout + missing_root.stderr):
            raise SystemExit("missing root-qualified path was not rejected with a missing-name diagnostic")

        member = source / "ImportMissingMember.scala"
        member.write_text(
            "package fixture\nimport _root_.middle.deep.third.Missing\nobject ImportMissingMember { val value = Missing }\n",
            encoding="utf-8",
        )
        missing_member = compile_sources(java_home, classpath + os.pathsep + str(output), root / "missing-member", [member])
        diagnostic = missing_member.stdout + missing_member.stderr
        if missing_member.returncode == 0 or "Missing" not in diagnostic:
            raise SystemExit("valid intermediate package did not distinguish a missing selected member")

        object_collision = source / "NameCollisionObject.scala"
        object_collision.write_text(
            "package collision\nobject Target { class FromObject }\n",
            encoding="utf-8",
        )
        collision_result = compile_sources(
            java_home,
            classpath + os.pathsep + str(output),
            root / "name-collision",
            [source / "NameCollisionPackage.scala", object_collision],
        )
        collision_diagnostic = collision_result.stdout + collision_result.stderr
        plain_collision_diagnostic = re.sub(r"\x1b\[[0-9;]*m", "", collision_diagnostic)
        if collision_result.returncode == 0 or not all(
            fragment in plain_collision_diagnostic
            for fragment in ("Target is already defined as package", "collision.Target")
        ):
            raise SystemExit(
                "Scala 3.9 object/package collision diagnostic changed:\n" + collision_diagnostic
            )

        nested_member = source / "ImportRelativeNested.scala"
        nested_member.write_text(
            "package a.consumer\nimport b.Local.Nested\nobject ImportRelativeNested { val value: Nested = new Nested }\n",
            encoding="utf-8",
        )
        relative_priority = compile_sources(
            java_home,
            classpath + os.pathsep + str(output),
            root / "relative-priority",
            [nested_member],
        )
        priority_diagnostic = relative_priority.stdout + relative_priority.stderr
        if relative_priority.returncode == 0 or "Nested is not a member of object a.consumer.b.Local" not in priority_diagnostic:
            raise SystemExit("unrooted import did not retain Scala 3.9 source-relative package lookup priority")

        parent_scope = source / "ImportParentPackageNotScope.scala"
        parent_scope.write_text(
            "package a.consumer.deep\nimport b.Local.Relative\nobject ImportParentPackageNotScope { val value = Relative }\n",
            encoding="utf-8",
        )
        parent_lookup = compile_sources(
            java_home,
            classpath + os.pathsep + str(output),
            root / "parent-package-scope",
            [parent_scope],
        )
        parent_diagnostic = parent_lookup.stdout + parent_lookup.stderr
        if parent_lookup.returncode == 0 or "Relative is not a member of object b.Local" not in parent_diagnostic:
            raise SystemExit("Scala 3.9 unexpectedly searched an enclosing package for an import qualifier")

        collision = source / "ImportSourceRelativePriority.scala"
        collision.write_text(
            "package a.consumer\nimport b.Local.Root\nobject ImportSourceRelativePriority { val value: Root = new Root }\n",
            encoding="utf-8",
        )
        relative_priority = compile_sources(
            java_home,
            classpath + os.pathsep + str(output),
            root / "source-relative-priority",
            [collision],
        )
        relative_diagnostic = relative_priority.stdout + relative_priority.stderr
        if relative_priority.returncode == 0 or "Root is not a member of object a.consumer.b.Local" not in relative_diagnostic:
            raise SystemExit("Scala 3.9 did not prefer the lexical source package when both paths exist")

    print(
        "scala=3.9.0 positive_imports=class,object,object_member,rename,_root_,root_package_from_nested_context,current_package_relative,stacked_package_parent_scope,descendant_only_intermediate,imported_qualifier_shadows_package "
        "negative_imports=absent_root,missing_selected_member,source_relative_wins_collision,dotted_package_parent_scope_not_searched,object_package_name_collision,lexical_term_missing_member_no_root_fallback;root_fallback=success "
        "root_qualification=bypasses_relative_search;lexical_term_binding=normal_priority"
    )
    return 0


if __name__ == "__main__":
    main()
