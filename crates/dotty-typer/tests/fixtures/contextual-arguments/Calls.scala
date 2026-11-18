class Calls:
  def need(using n: Int): Int = n
  def forward(using n: Int): Int = need
  def explicit(using n: Int): Int = need(using n)
  def f(x: Int)(using n: Int): Int = n
  def local: Int =
    given instance: Int = 7
    f(1)
  def legacy(implicit n: Int): Int = n
  def positional: Int = legacy(9)
  def overrideAmbiguity: Int =
    given one: Int = 1
    given two: Int = 2
    need(using 9)

class Base
class Child extends Base
class Priority:
  given broad: Base = new Base
  given narrow: Child = new Child
  def need(using x: Base): Base = x
  def read: Base = need
