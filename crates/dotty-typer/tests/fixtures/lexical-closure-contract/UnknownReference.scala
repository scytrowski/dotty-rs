package lexicalclosure

object UnknownReference:
  val unknown: Int => Int = x => x + missing
