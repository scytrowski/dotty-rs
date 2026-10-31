package example

object RustArtifactConsumer:
  def main(args: Array[String]): Unit =
    val first = new Counter(7)
    val second = new Counter(3)
    val pair = new Pair(first, second)
    assert(pair.leftValue() == 7)
    assert(pair.rightValue() == 3)
    first.set(42)
    assert(pair.leftValue() == 42)
    assert(pair.rightValue() == 3)
    second.set(-9)
    assert(pair.leftValue() == 42)
    assert(pair.rightValue() == -9)

    val reversed = new Pair(second, first)
    assert(reversed.leftValue() == -9)
    assert(reversed.rightValue() == 42)

    val aliased = new Pair(first, first)
    first.set(70)
    assert(aliased.leftValue() == 70)
    assert(aliased.rightValue() == 70)
    println("scala39-rust-dual-reference-consumer=ok")
