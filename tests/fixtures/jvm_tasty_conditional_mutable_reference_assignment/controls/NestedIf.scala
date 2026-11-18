package example

class PairFactory:
  def create(firstSeed: Int, secondSeed: Int, chooseSecond: Boolean, nextValue: Int): Pair =
    val first: Counter = new Counter(firstSeed)
    val second: Counter = new Counter(secondSeed)
    var selected: Counter = first
    if chooseSecond then
      if nextValue == 0 then
        selected = second
      else
        selected = first
    else
      selected = first
    selected.set(nextValue)
    new Pair(first, second)
