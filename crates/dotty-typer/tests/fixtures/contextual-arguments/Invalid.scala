class Invalid:
  def need(using n: Int): Int = n
  def empty(using n: Int): Int = need()
  def missing: Int = need
  def ambiguous: Int =
    given one: Int = 1
    given two: Int = 2
    need
  def absent(using n: scala.util.NotGiven[Int]): Int = 1
  def present(using n: Int): Int = absent

class Choice
class LeftDep
class RightDep
object Choice:
  given left: LeftDep => Choice = new Choice
  given right: RightDep => Choice = new Choice
class EqualPriority:
  def take(using c: Choice): Int = 1
  def unresolved(using x: LeftDep, y: LeftDep, z: RightDep): Int = take
