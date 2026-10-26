# ASM classfile member oracle

Compiled with Scala 3.9.0 at pinned revision
`777528f19a58e794c9954a42f433373472ec57f8` and JDK 21.0.8. The classpath
must include the pinned Scala compiler dependencies, including
`scala-asm-9.9.0-scala-1.jar`:

```sh
"$JAVA_HOME/bin/java" -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CLASSPATH" -d target/asm-member-oracle \
  crates/dotty-typer/tests/fixtures/asm-member-resolution/AsmMemberResolution.scala
```

The positive fixture typechecks calls to `AbstractInsnNode.getOpcode()` and
`ClassReader.readByte`, `readUTF8`, and `readShort` with their declared result
types. These are the call shapes at the focused Scala roots
`LocalOpt.scala#tree=3134` and `InlineInfoAttribute.scala#tree=420`.

The negative fixture is rejected by the same compiler and classpath:

```sh
"$JAVA_HOME/bin/java" -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CLASSPATH" -d target/asm-member-oracle \
  crates/dotty-typer/tests/fixtures/asm-member-resolution/AsmMemberResolutionNegative.scala
```

Scala reports that `noSuchOpcodeMethod` is not a member of
`scala.tools.asm.tree.AbstractInsnNode`. This establishes the pinned source
oracle's positive and negative name-resolution behavior. It does not yet
identify which lookup or class-completion operation fails in the Rust Typer;
that requires a focused Rust reproducer before implementation.
