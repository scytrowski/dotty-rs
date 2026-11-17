package example

class PairFactory:
  def create(firstSeed: Int, secondSeed: Int, chooseSecond: Boolean, nextValue: Int): Pair =
    val first: Counter = new Counter(firstSeed)
    val second: Counter = new Counter(secondSeed)
    var selected: Counter = first
    if chooseSecond then
      selected = new Counter(nextValue)
    else
      selected = first
    selected.set(nextValue)
    new Pair(first, second)
