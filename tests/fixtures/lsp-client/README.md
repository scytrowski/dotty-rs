# LSP integration fixture

This standalone Rust package exercises the public `dotty` API as a language
server would. Cargo builds a test executable named `dotty-lsp`. The fixture
owns its workspace and lockfile, so its LSP dependencies stay outside the
compiler library. The production language server belongs with the future
compiler in a separate repository.

Set `JAVA_HOME` to a JDK 21 installation for the binary dependency test.
From this directory, run the protocol regression with:

```sh
cargo test --locked
```

The integration test starts the executable and exchanges LSP messages over
standard input and output. It opens two CRLF documents, edits a call in memory
to select a different overload, and checks the declaration and name ranges in
the other document. A type error in a sibling method produces a versioned
diagnostic while navigation still works. Repairing the method clears the
diagnostic. The test runs with UTF-8 and UTF-16 positions and an emoji before
the call. A second scenario checks an inferred field in Consumer.scala against
a method in Provider.scala in both file orders. Signature and body checking
report one error at `provider.value(1)`. Editing it to `provider.value()`
clears the diagnostic. The fixture routes direct semantic diagnostics by
`SemanticDiagnostic::source`. It keeps the typed payload until publication,
including warnings from successful members of a partial class.
The fixture calls `finish_with_body_work` and checks signature-ready members.
A rejected signature supplies its own marker. Independent checked methods
retain their navigation bindings while a dependency is broken. The fixture
records each outcome in `CheckedClassBodies` and builds one `ResolvedMemberIndex`
for the current document versions. Both `textDocument/definition` and
`textDocument/references` read this index after the semantic arenas are released.
Both requests use `ResolvedMemberIndex::symbol_at`.
For `textDocument/hover` the fixture calls `symbol_info_at` on the same revision.
It formats the typed class, value or method locally and returns the written name
range in the negotiated encoding. Editing a call changes the selected signature;
a rejected call returns JSON `null` until repaired.

For `textDocument/signatureHelp`, the client passes the insertion position to
`ResolvedMemberIndex::call_signature_at`. It renders the selected method and
its formal parameters, highlighting the formal bound to the written argument at the cursor.
The stdio regression checks nested calls, a change of overload after an edit,
and a rejected sibling call in both negotiated encodings. The request covers
completed ordinary positional and named applications with a committed source target. For an
unfinished call the fixture keeps the parser diagnostics, then builds an
`IncompleteCallIndex` at the end of the revision. It returns the compatible
signatures in lookup order. When a written name selects different formal
ordinals, each signature carries its own `activeParameter`. Agreement uses
the shared top-level field. Hover continues to require a
committed target. The four-parameter stdio test keeps two overloads through
the first two arguments and narrows them when the third argument is typed.
A second stdio test keeps the call unchanged, edits the local third argument’s
initializer from `2` to `false`, then undoes the edit. The fourth formal
changes from `Boolean` to `Int` and back in both position encodings. The test
also exercises the same call inside `val result = …`. Completing the fourth
argument and closing the call selects the predicted overload and clears the
syntax diagnostic.

For first-clause ordinary source defaults the fixture displays `= …` from the
parameter's registered getter. The provider-edit stdio test removes a default,
checks the missing required formal on the unchanged call and restores the
signature by undoing the edit in either encoding.

Bare calls follow lexical shadowing. Adding `val pick = 0` before the call
suppresses its signature help until undo. Member calls also follow access at
the use site. Changing one overload in `Ops.scala` to `private` removes it
from the other open document’s candidates. The other overload stays visible
for compatible prefixes, and undo restores the original suggestions at the
same cursor position.

The references request accepts positions on direct calls, supported field accesses, method parameter
reads, ordinary method-local values and variables, and their declaration names. Field queries cover public selections,
bare uses of the enclosing class’s fields, and its private body fields written
with a bare name or `this`. The stdio tests rename a parameter so it shadows a
field and then undo the edit. The bare read changes from a field to a parameter at the same byte offset,
while the explicit and cross-file field references stay available. Overload tests
check the exact parameter token and its references in UTF-8 and UTF-16.
Tests for nested blocks check distinct local declarations and reads before and
after a sibling method fails. Renaming a local to shadow a parameter redirects
a read at the same offset; undo restores both reference lists.
The method signature scenario follows both type annotations in `def copy(x: Base): Base = new Base()`.
Editing only the parameter type to `Peer` moves one reference. The result and allocation continue to
point to `Base`. Undo restores all three references in both UTF-8 and UTF-16.
The construction scenario follows the class, argument and method in `new Base(start).read()`.
An edit to `new Peer(start)` retargets the allocation and `read` method. Undo restores
both links, and a separate subclass retains its reference to `Base` throughout.
The superclass scenario follows `Base` in `extends Base(start)` into its defining file.
An edit to `Peer(start)` and undo move one reference while preserving the other subclass
and the argument definition in both position encodings.
The index includes committed parent argument bindings from the header phase.
An edit between two constructor parameters in a parent application moves that
reference between declarations while retaining the initializer reads in both files.
The `includeDeclaration` option adds the defining name. An edit that moves one
call between overloads preserves the other caller, even with a failing sibling. The fixture renders the message at
publication. Two inferred fields with matching bad calls retain independent
markers as each field is repaired.

The fixture uses `ClassHeaderPhase::complete_all` to visit headers and retry released waiters. Its final report supplies
the classes eligible for member checking, dependency blockages and the original
constructor and header errors. A protocol test checks that the
marker on the parent argument clears after an unsaved change to its type.
Another test opens a constructor with an unknown type after an emoji. It checks
the range in both position encodings, repairs the type in memory and resolves
a call to the same overload in a separate file before and after the repair.
A class waiting on the broken constructor stays free of secondary diagnostics.
Its own call becomes navigable once the base class is repaired.
When no source range is available, it sends the message to the editor log
and leaves the document free of a marker for that error.

The fixture passes its session resolver to `MemberSignaturePhase::complete_all` for members selected from ready classes.
A protocol test breaks the initializer of `seed`, checks the original diagnostic
and an independent sibling error, then repairs the initializer and verifies the
cross-file definition link in both UTF-8 and UTF-16. Completion wakes
direct waiters. Both phases produce a `SignatureBlockageReport` with the
source positions of the cycle participants. A stdio regression creates and
repairs a cross-file inheritance cycle. Only its two participants receive
cycle markers. A third document containing a downstream class keeps its
independent call error through both edits.

The field-declaration scenario opens `private[Missing] val hidden: Int 1`.
It checks for one parser report at the literal. Inserting `=` exposes the
naming report at `Missing`, and removing the qualifier clears the document.
A subsequent edit puts two declarations on one line. Inserting a semicolon
clears the resulting separator diagnostic.
Both clients bind the result of `dotty::parse_source` with `DetachedSource::bind`
to obtain owned `SyntaxDiagnostic::Lexer` and `SyntaxDiagnostic::Parser` values.
Both clients use `ParsedSource::prepare_classes` to collect prepared files for
`NamedClassSources` and retain the original `PreparationDiagnostic` payloads.
A stdio test inserts a trait between two classes, checks its diagnostic after
an emoji in both position encodings, and follows a call from a second document
to the overload in the later class. Editing the trait into a class clears the
marker and preserves the link. It uses this collection to resolve signatures by
symbol and find the caller and target files for direct-call navigation.
The stdio test continues navigating between two clean files while a third
moves through parser rejection, namer rejection and repair.
The server renders preparation diagnostics at publication and sends each
available stable code in the LSP diagnostic's `code` field. A cross-file
stdio test checks that navigation resumes on the first partially prepared
version, while the model still reports an omitted trait. The final edit
clears that report in both position encodings.

A separate protocol test opens a field with a missing equals sign and a trailing
unterminated comment. Closing the comment removes its marker while the parser
marker stays on the literal. Inserting the equals sign clears the document.
Reopening the comment produces a lexer diagnostic even when parsing is clean.
The test checks the document version and exact UTF-8 and UTF-16 positions.

For a positional call to a legacy implicit clause, analysis retains the typed
warning until publication. The stdio test checks its stable code, warning
severity and argument range alongside an independent argument type error. After
the type error is repaired, editing `f(1)` to `f(using 1)` clears the warning.

For a quick interoperability check with Neovim 0.10+, run from this directory:

```sh
cargo build --locked
DOTTY_LSP_BIN="$PWD/target/debug/dotty-lsp" nvim --clean --headless -l tests/editor.lua
```

The editor command jumps from a call in B.scala to the method name in A.scala.


The public-field stdio scenario reads and writes a constructor field. Editing
the writer's receiver type moves that usage to a body field in a different class.
A subsequent Boolean assignment reports the mismatched value and removes the
uncommitted write. The sibling method's read remains navigable in both UTF-8
and UTF-16.

The external dependency test compiles `demo.External` with `javac --release 21`.
The test passes the absolute class output directory in the fixture's optional
`initializationOptions.classDirectory`. The server combines that directory with
`$JAVA_HOME/jmods/java.base.jmod` and borrows this classpath for each fresh
analysis. After deleting only `External.class`, the test reopens the same
document to trigger a new analysis and checks the constructor diagnostic in
UTF-8 and UTF-16. The client also tests the underlying typed error before
publishing it.

For a checked `a.pick(right = right)`, definition, hover and references use
the retained member index for both tokens. The label targets the formal of
the selected overload; the operand targets the caller's local declaration.
The stdio test changes only the local initializer from `4` to `true` and
back. It verifies both reference lists and both definition ranges after each
edit in UTF-8 and UTF-16.

For rejected ordinary overloads, the client publishes one diagnostic on the call.
With `publishDiagnostics.relatedInformation`, it includes each candidate's operand
or label and its available provider declaration. The stdio fixture changes
`pick(right = true)` to `pick(flag = true)`, checks the repaired definition link,
then undoes the edit and verifies the restored evidence in UTF-8 and UTF-16.
