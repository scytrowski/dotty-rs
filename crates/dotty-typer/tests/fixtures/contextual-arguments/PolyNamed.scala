package inference
class Box[A]
class Fmt[A]
class Result[A, B]
class Client:
  def pair[A, B](using x: Box[A], y: Box[B]): Result[A, B] = new Result[A, B]
  def choose[A](using value: Box[A], fmt: Fmt[A]): Box[A] = value
  def pos(b: Box[Int], f: Fmt[Int]): Box[Int] = choose(using b, f)
  def explicit(b: Box[Int], f: Fmt[Int]): Box[Int] = choose[Int](using fmt = f, value = b)
  def ordered(b: Box[Int], f: Fmt[Int]): Box[Int] = choose(using value = b, fmt = f)
  def reversed(b: Box[Int], f: Fmt[Int]): Box[Int] = choose(using fmt = f, value = b)
  def two(b: Box[Int], c: Box[Boolean]): Result[Int, Boolean] = pair(using y = c, x = b)
  def integer(): Box[Int] = new Box[Int]
  def boolean(): Box[Boolean] = new Box[Boolean]
  def effects(): Result[Int, Boolean] = pair(using y = boolean(), x = integer())
  def front[A](using fmt: Fmt[A], value: Box[A]): Box[A] = value
  def middle[A](using value: Box[A], fmt: Fmt[A], flag: Boolean): Box[A] = value
  def tail(b: Box[Int])(using f: Fmt[Int]): Box[Int] = choose(using value = b)
  def leading(b: Box[Int])(using f: Fmt[Int]): Box[Int] = front(using value = b)
  def gap(b: Box[Int])(using f: Fmt[Int]): Box[Int] = middle(using b, flag = true)
  def wide(b: Box[Int])(using f: Fmt[Int]): Any = choose(using value = b)
  def fallback[A](using value: Box[A], fmt: Fmt[A] = new Fmt[A]): Box[A] = value
  def defaulted(b: Box[Int]): Box[Int] = fallback(using value = b)
  def shadow(b: Box[Int])(using f: Fmt[Int]): Box[Int] = fallback(using value = b)
  def overrideDefault(b: Box[Int])(using f: Fmt[Int]): Box[Int] = fallback(using fmt = f, value = b)
