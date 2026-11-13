package issue2282oracle

case class Rightish[+A, +B](value: B)
case class Leftish[+A, +B](value: A)

object Probe:
  def right[A](value: A): Rightish[Nothing, A] = Rightish(value)
  def left[B](value: B): Leftish[B, Nothing] = Leftish(value)
