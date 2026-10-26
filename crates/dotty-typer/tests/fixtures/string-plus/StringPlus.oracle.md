# Scala 3.9 `String.+` oracle

The pinned Scala 3.9.0 compiler installs `String_+` as a compiler-known
synthetic method on `java.lang.String`: `Definitions.scala` declares it with
`methOfAny(StringType)` and `Final`. Its canonical source signature is
`java.lang.String.+(Any): java.lang.String`; the method is not present in the
JDK `String.class` member table.

The positive source is accepted and its result type is `String`:

```scala
object StringPlusOracle:
  def concat(name: String, desc: String): String = name + desc
```

The negative control is rejected because unrelated reference classes do not
inherit the synthetic String operator:

```scala
class Token
object StringPlusNegativeOracle:
  def concat(left: Token, right: Token): Token = left + right
```

Reproduce with the pinned checkout at revision
`777528f19a58e794c9954a42f433373472ec57f8` and cached Scala 3.9.0/JDK 21.0.8
artifacts:

```sh
classpath=$(find target/scala39-maven -name '*.jar' -printf '%p:')
java=target/temurin21-1525/bin/java
mkdir -p target/string-plus-oracle
cat > target/string-plus-oracle/StringPlus.scala <<'EOF'
object StringPlusOracle:
  def concat(name: String, desc: String): String = name + desc
EOF
"$java" -cp "$classpath" dotty.tools.dotc.Main \
  -classpath "$classpath" -d target/string-plus-oracle \
  target/string-plus-oracle/StringPlus.scala
cat > target/string-plus-oracle/StringPlusNegative.scala <<'EOF'
class Token
object StringPlusNegativeOracle:
  def concat(left: Token, right: Token): Token = left + right
EOF
if "$java" -cp "$classpath" dotty.tools.dotc.Main \
  -classpath "$classpath" -d target/string-plus-oracle \
  target/string-plus-oracle/StringPlusNegative.scala; then
  echo 'expected the unrelated Token operator to be rejected' >&2
  exit 1
fi
```

The implementation test uses an exact `java.lang.String` classfile identity,
asserts the selected symbol's owner and `Any => String` signature, and covers a
same-program source class negative control. It also verifies rollback when
typing the right operand fails before retrying a successful concatenation.
