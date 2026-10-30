package example

object WrongSecondType:
  def construct(first: Counter): Pair = new Pair(first, 1)
