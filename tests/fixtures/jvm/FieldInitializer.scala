class Answer:
  val seed: Int = 1
  def value(): Int = 42
  def select(flag: Boolean, x: Int, y: Int): Int =
    if flag then x else y
