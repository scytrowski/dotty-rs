# `scala.Predef.String` provenance and resolution audit (#2025)

This audit records the two original `Animal.sound` / `Dog.sound` completion
failures before A2. Its deterministic, diagnostic-only rows are in
[`predef-alias-audit.tsv`](predef-alias-audit.tsv); that TSV remains a frozen
historical report. Current behavior is checked separately by the focused
classloader integration test. The historical 14-unit / 126-row reports and
`docs/classloader-semantic-tasty-convergence-ii/member-completion.tsv` remain
unchanged.

## Frozen inputs

- Base: fresh `main` at `be23db8383fa8537cb9d5b5ea44a66161841b356`.
- Scala reference source: [Scala 3 `Predef.scala` at revision
  `777528f19a58e794c9954a42f433373472ec57f8`](https://github.com/scala/scala3/blob/777528f19a58e794c9954a42f433373472ec57f8/library/src/scala/Predef.scala).
  The declaration is `object Predef` (line 108) and the source documents
  `type String = java.lang.String` (line 171). The alias is a Scala source type
  alias, not a request for a JVM class named `scala.Predef.String`.
- `Predef.tasty`: format 28.9, SHA-256
  `2faef7420109c4e44074ab0385dce2c31e751fd992621fd1de312eed6c362fb2`.
- `Animal.tasty`: SHA-256
  `9f205fe92b667f4f83a6a3d03d6dd45ec8f22f979e5acd73608e75ba1f2af9b3`;
  `Dog.tasty`: SHA-256
  `9fb989680c0fbc7811ca1343437f223f4fd60b368d95fc8a0e1fbc49023cd13a`.
  Both are format 28.9 and pinned to the same Scala 3.9.0 fixture revision.
- JDK: Temurin `21.0.13+9-LTS`; `java.base.jmod` SHA-256
  `fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`.
- Ordered classpath for the original method reproduction and JDK-present alias
  probe: `dotty-classloader/tests/fixtures/tasty_sample`,
  `dotty-tasty/tests/fixtures/scala3-library`,
  `dotty-tasty/tests/fixtures/path_dependent`,
  `dotty-tasty-unpickler/tests/fixtures/semantic`, then the JDK 21 `jmods`.
  The missing-JDK control uses the same ordered fixture entries and omits only
  `jmods`.

## Declaration identity and first failures

The indexed Predef TASTy has `Predef` object at AST address 107,
`Predef$` ModuleClass at 128, and a `String` TYPEDEF at 378. The alias enters
as `SymbolKind::TypeAlias`, in the Type namespace, public, and owned by the
`Predef$` module class. `Predef` is a distinct Term-namespace Object owned by
package `scala`; it is the companion object identity, not the TYPEDEF owner.
The alias TYPEDEF has no private/protected modifier. Its original target tree
uses the name-table components `java`, `lang`, and `String`, yielding
`java.lang.String`. These are the decoded TASTy identities and tree, not
identities inferred from classpath filenames.

For the original owners, `Animal` TYPEDEF@4 contains `sound` DEFDEF@22 and its
first unresolved type member is at address 27. `Dog` TYPEDEF@4 independently
contains `sound` DEFDEF@36 and fails at address 41. Both requests select
`scala/Predef.tasty` from the Scala library fixture path and then report
`UnresolvedMember` for Type-namespace `String`. The missing
`scala/Predef/String` and `scala/Predef$String` classpath candidates are
observations from the earlier member audit; they are not the alias declaration
or evidence that a class resource should exist.

For both original method sites, the test records the first canonical request
and ordered classpath observations. The resolver selects `Predef.tasty` but no
`SymbolResolver::module_class_of_object` or
`SymbolResolver::complete_module_class` callback is observed before the first
typed error (`module_identity_trace=-`). This separates resource selection
from successful module-identity projection: the binary proves the companion
ModuleClass and owned alias exist, while this method-resolution attempt does
not report that identity through the resolver contract before failing.

Completing Predef's `String` TYPEDEF directly isolates its target path. With
JDK 21 configured, the bounded resolver requests term member `lang` on prefix
`java`, observes no `java/lang` standalone class resource, and returns
`UnresolvedMember` at TASTy address 383. Without the JDK classpath, the same
alias target stops earlier with `UnresolvedPackage { package: "java" }` at
address 385. The control therefore reaches a distinct dependency boundary;
it does not repeat the Animal/Dog `String` failure. Neither result establishes
that the JDK lacks `java.lang.String`; it establishes that this resolver path
has not completed the nested package/class target. The resolver must follow the
package and class identities represented by the target tree instead of
substituting a same-spelled alias or guessing a `Predef/String` resource.

The wrong-owner control compares the alias owner `SymbolId` with the canonical
`ModuleClass` at TASTy address 128. It rejects the indexed `Predef` term object
at address 107 and a distinct same-named `Predef$` `ModuleClass`, while
accepting only the canonical indexed symbol. The diagnostic keeps names for
readability (`ForeignOwner { requested: "Predef$", actual: "Predef" }` for the
term-object case), while the check validates the expected owner's type
namespace and module-class role. The alias remains `Missing` throughout. The
existing member-audit foreign-owner regression independently checks a Dog
method address cannot be accepted as an Animal member. All controls fail closed
without name-only substitution.

## Bounded A2 / A3 contract

The existing core representation is sufficient for an ordinary alias:
`SymbolKind::TypeAlias` and `Type::AliasingBounds { alias }` already exist, and
the TASTy type-tree reader can represent a target `TypeRef`. No new core model
is indicated by this evidence.

- **A2 — TASTy semantic projection** belongs in `dotty-tasty-unpickler` at
  `TastyUnpickler::complete_symbol` / TYPEDEF completion. Preserve the
  `scala.Predef$` owner, Type namespace, visibility, and alias target as the
  existing alias-bounds representation. Completion must remain transactional:
  failure resolving the target must restore the TYPEDEF state and all type
  allocations so retry is valid.
- **A3 — canonical external resolution** belongs in
  `dotty-classloader/src/resolver.rs`: `prefix_target_bounded`,
  `resolve_member_inner`, and module identity handling in
  `try_ensure_module_identity`. Resolve the `scala.Predef` term object to its
  actual ModuleClass, select its owned Type-namespace `String` TYPEDEF, then
  resolve the alias target through the JDK package/class path. Keep identity
  and package/classpath/cache mutations under the resolver's checkpoint and
  rollback boundary. Do not search for or synthesize a standalone
  `Predef$String` class.

This is a measured handoff, not a claim that the current resolver completes the
alias. It does not change resolver behavior or broaden classloader acceptance.

## A2 — bounded classloader declaration publication

The classpath resolver now projects one declaration while completing the
canonical `scala/Predef$` module class. It reuses the selected `Predef.tasty`
resource and validates the module class at address 128, then selects the direct
TypeDef at address 378 from that module template. The resulting core symbol is
`TypeAlias` / Type namespace / owner `Predef$`; it takes the fixture's flags,
visibility and TASTy origin. The resolver exposes that same symbol through its
existing type-member lookup.

The alias target is retained from the address-indexed RHS as the typed binary
path `java/lang/String` in the resolver's decoded-module cache. A2 does not
load or complete that class. The published alias symbol retains
`SymbolInfo::Missing` until follow-on resolver work can connect that target to
its canonical class identity. The alias SymbolId remains distinct from the
target class identity. Module completion and its declaration scope use the
existing `LoadingSession` journal: rollback removes the alias scope and module
completion, and a retry in the same resolver session re-publishes one alias
under the canonical module-class SymbolId.

The original `Animal.sound` and `Dog.sound` sites now complete their method
signature construction through a symbol-backed `TypeRef` to this canonical
`scala.Predef$.String` alias. The integration assertions check the original
DEFDEF addresses 22 and 36, the alias kind and owner path, and that alias
`SymbolInfo` remains `Missing` after each signature completes. This is
signature construction only: the separate direct `complete_symbol(378)` probe
still attempts target resolution and independently returns the pinned failure
at target address 383 with JDK 21, or the missing-package error at address 385
without the JDK. Neither result claims that `java.lang.String` has been loaded
or that either full Animal/Dog completion path has been accepted.

No general TypeDef/opaque/higher-kinded alias projection was added. Other
module TASTy files and same-spelled term members continue through their
existing paths; a malformed or ambiguous indexed Predef alias is a typed TASTy
projection failure rather than a guessed class or fallback type.
