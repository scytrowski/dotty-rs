package me.cytrowski.tastyfixtures.semantic

// One literal per constant kind. Each initializer is written into the TASTy
// file as the constant's own node, which is also what a constant type is.
object Constants {
  class Item

  val unit: Unit = ()
  val yes = true
  val no = false
  val nothing: Null = null
  val byte: Byte = 7
  val short: Short = -300
  val char = 'x'
  val maxChar: Char = '￿'
  val surrogate: Char = '\uD800'
  val int = 42
  val long = 1234567890123L
  val float = 1.5f
  val double = 2.25
  val negativeZero = -0.0
  val string = "hello"
  val unicode = "héllo ✓"
  val empty = ""
  val cls = classOf[Item]
  val nested = classOf[List[Item]]
}
