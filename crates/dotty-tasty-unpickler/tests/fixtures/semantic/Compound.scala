package me.cytrowski.tastyfixtures.semantic

// Compound types built from other types. Everything they mention lives in
// this unit, so the references inside them resolve without a classpath.
object Compound {
  trait A
  trait B
  trait C
  class Item
  class Box[T]
  class Pair[K, V]

  class Base {
    def hello: Int = 1
  }

  class Derived extends Base {
    override def hello: Int = super[Base].hello
  }

  def single(x: Box[Item]) = x
  def several(x: Pair[Item, Box[Item]]) = x
  def nested(x: Box[Box[Item]]) = x
  def both(x: A & B): A & B = x
  def chained(x: (A & B) & C): (A & B) & C = x
  def either(x: A | B): A | B = x
  def deeper(x: A | (B | C)): A | (B | C) = x
  def consume(x: => Item): Item = x
  val inferredBox = new Box[Item]
  val inferredPair = new Pair[Item, Box[Item]]
  val inferredNested = new Box[Box[Item]]
  def inferredAnd = (null: A & B)
  def inferredOr = (null: A | B)
  def inferredChained = (null: (A & B) & C)
  def inferredDeeper = (null: A | (B | C))
  def takesThunk(f: (=> Item) => Item): Item = null
  def lazily(x: => Item) = () => x
  val byNameFn = (x: Int) => consume(null)
  def overridden = consume(null)
}
