package genericconversions

class Box[A]
class Wrapped[A]
class Child extends Box[Int]

class Client:
  implicit def wrap[A](x: Box[A]): Wrapped[A] = new Wrapped[A]
  def accept(x: Wrapped[Int]): Wrapped[Int] = x
  def result(x: Box[Int]): Wrapped[Int] = x
  def argument(x: Box[Int]): Wrapped[Int] = accept(x)
  def inherited(x: Child): Wrapped[Int] = x

class Factory:
  implicit def make[A, B](x: Box[A]): Wrapped[B] = new Wrapped[B]
  def fromExpected(x: Box[Int]): Wrapped[Boolean] = x

class Specialized:
  implicit def generic[A](x: Box[A]): Wrapped[A] = new Wrapped[A]
  implicit def special(x: Box[Int]): Wrapped[Int] = new Wrapped[Int]
  def specialized(x: Box[Int]): Wrapped[Int] = x

class Pair[+A, B]
class Impl[A, B] extends Pair[A, B]
class Projected:
  implicit def pair[A, B](x: Box[A]): Impl[A, B] = new Impl[A, B]
  def split(x: Box[Int]): Pair[Any, Boolean] = x
