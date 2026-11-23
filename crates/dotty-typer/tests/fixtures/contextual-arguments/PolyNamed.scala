package inference
class Box[A]
class Fmt[A]
class Client:
  def choose[A](using value: Box[A], fmt: Fmt[A]): Box[A] = value
  def pos(b: Box[Int], f: Fmt[Int]): Box[Int] = choose(using b, f)
  def explicit(b: Box[Int], f: Fmt[Int]): Box[Int] = choose[Int](using fmt = f, value = b)
  def ordered(b: Box[Int], f: Fmt[Int]): Box[Int] = choose(using value = b, fmt = f)
