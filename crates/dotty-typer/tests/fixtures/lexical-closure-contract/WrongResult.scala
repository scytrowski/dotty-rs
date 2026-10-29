package lexicalclosure

object WrongResult:
  val wrong: Int => Boolean = x => x + 1
