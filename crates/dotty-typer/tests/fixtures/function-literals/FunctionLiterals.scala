object FunctionLiterals:
  val zero: () => Int = () => 1
  val one: Int => Int = (x: Int) => x
  val many: (Int, Int) => Int = (x: Int, y: Int) => x
  val block: Int => Int = (x: Int) =>
    val local: Int = x
    local

  def consume(function: Int => Int): Int = function(1)
  val argument: Int = consume((x: Int) => x)
