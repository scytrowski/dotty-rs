# JDK 21 JPMS metadata compatibility gate

## Result

**GO** for typed Java 21 `Module`, `ModulePackages`, and `ModuleMainClass`
decoding, descriptor validation, and the read-only `JdkModuleCatalog` on the
JDK 21.0.8+9 reference image. The opt-in integration gate extracts and decodes
the exact `classes/module-info.class` from `java.base.jmod`,
`java.net.http.jmod`, and `jdk.jshell.jmod`, validates the `ACC_MODULE` shape,
and compares every typed `Module` row and optional companion attribute with
the corresponding catalog entry. It also compares every `jmod describe`
field that is observable for these descriptors.

| JDK 21 module | Verified coverage |
| --- | --- |
| `java.base` | Name/version, module flags, requires, unqualified and qualified exports, uses/provides, packages, JMOD origin |
| `java.net.http` | Name/version, mandated requires, unqualified export, packages, JMOD origin |
| `jdk.jshell` | Name/version, transitive and mandated requires, exports, service uses/provides, packages, JMOD origin |

All three descriptors omit `ModuleMainClass`; the gate confirms that absence
remains `None`. The pinned raw classfile fixtures and `module_info_oracle`
tests separately cover present `ModulePackages`, present `ModuleMainClass`,
and deliberately absent companion attributes, including byte-for-byte typed
codec round trips.

## Reproduce

Use Eclipse Temurin 21.0.8+9 or another Java 21 JDK. The gate is opt-in and
does not download tools or run in default CI:

```sh
export JAVA_HOME=/path/to/jdk-21
export DOTTY_JDK_JMODS="$JAVA_HOME/jmods"
cargo test -p dotty-classloader --test real_jdk_class_path --locked -- --ignored
```

The `checks_typed_jdk21_descriptors_against_jmod_describe` test invokes the
matching `jmod` CLI, extracts each exact module descriptor, and fails if the
JDK tools, modules, typed fields, or CLI observations are unavailable or do
not agree. Existing catalog and class lookup tests run in the same command.

## What the independent CLI establishes

`jmod describe` verifies its printed module name/version, requires (including
transitive/mandated markers), exports and qualified targets, opens, service
uses/provides, and package contents. Its `contains` listing is checked against
the non-exported class-package inventory from `jmod list`. This physical
archive inventory is not assumed to be identical to `ModulePackages`: for
example, JDK 21.0.8 `java.base` contains `jdk.internal.jrtfs` in the archive
although it is not listed in that descriptor attribute.

The command does not print raw `ModulePackages` membership as a separate
attribute, `ModuleMainClass`, all encoded flag bits, or constant-pool indices.
Those are checked against the decoded raw descriptor and JVMS-shaped fixtures,
not inferred from CLI text. JDK-specific `ModuleTarget` and `ModuleHashes`
remain preserved opaque attributes. The current JDK descriptors tested here
have no main class; a present `ModuleMainClass` is covered by the pinned
`fixture.main` classfile oracle.

## Regression gates and boundaries

The classfile oracle tests cover wrong references, invalid flags and names,
duplicate `Module*` attributes/directives, malformed and truncated payloads,
invalid `ACC_MODULE` shape, and companion-attribute constraints. Classloader
fixtures cover missing descriptors, malformed classfiles, invalid descriptors,
bad projected references, trailing bytes, duplicate module identities, and
filename/name mismatches. These return typed errors with JMOD origin/context;
they are separate from the real-JDK positive integration gate.

The catalog check keeps deterministic descriptor-name ordering and exact
`ClassOrigin::Jmod` provenance. Existing classpath tests continue to cover
sorted first-match-wins lookup, error propagation, direct and descendant
package evidence, safe binary names, archive-backed reads, and concurrent
access. Frontend-only typing remains independent of `JAVA_HOME`; JPMS metadata
stays in the classfile/classloader APIs and is not added to the source AST or
typer interfaces. Existing JVM compilation and execution gates remain
unchanged.

The benchmark in `classpath-jdk-performance-results.md` records the real JDK
21 catalog cost and file-descriptor behavior. The matched original-driver
initialization median was 17.754 ms before and 17.543 ms after; peak RSS was
13,210 KiB and 13,202 KiB, and both used 73 FDs. A first whole-catalog read
had a 5.437 ms median. Independent classpath instances used 73/142/280 FDs
for 1/2/4 instances; metadata calls retained zero additional handles and
dropping the instances returned to the four-FD process baseline. A 256-FD
limit rejects four independent JDK classpaths, so caller-managed classpath
sharing remains a separately scoped follow-up.

This GO is limited to descriptor decoding/validation and read-only catalog
inspection. The implementation does not provide JIMAGE or `ct.sym` loading,
`--release` API targeting, module graph resolution, readability/accessibility
or exports enforcement, runtime module loading, or automatic JDK detection.
