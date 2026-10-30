package example

object WrongForeignOwner:
  def construct(first: Counter): Pair = new Pair(first, new foreign.Counter())
