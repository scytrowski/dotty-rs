package lexicalclosure

object LocalValues:
  def offset: Int => Int =
    val bias: Int = 3
    x => x + bias

  def two: Int => Int =
    val first: Int = 1
    val second: Int = 2
    x => x + first + second + first

  def shadowed: Int => Int =
    val value: Int = 100
    (value: Int) => value

  def nestedShadow: Int => Int =
    val value: Int = 100
    {
      val value: Int = 3
      x => x + value
    }
