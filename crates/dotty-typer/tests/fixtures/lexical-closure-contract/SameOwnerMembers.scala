class Calculator:
  val bias: Int = 2
  def adjust(x: Int): Int = x + bias
  def lifted: Int => Int = x => adjust(x)
  def explicit: Int => Int = x => this.adjust(x)
  def withBias: Int => Int = x => x + bias

trait Adapter:
  def apply(x: Int): Int
  def lifted: Int => Int = x => apply(x)
