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

class NegativeEvidence:
  import scala.util.NotGiven
  def absent(using n: NotGiven[Base]): Int = 1
  def free: Int = absent
  def ambiguous(using a: Base, b: Base): Int = absent

class Overloaded:
  def one(using n: Int): Int = n
  def one(using b: Boolean): Boolean = b
  def from(x: Int)(using n: Int): Int = n
  def from(x: Int)(using b: Boolean): Boolean = b
  def pick(using n: Int): Int = one
  def take(using b: Boolean): Boolean = from(1)
