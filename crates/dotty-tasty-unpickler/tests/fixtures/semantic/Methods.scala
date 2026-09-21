package me.cytrowski.tastyfixtures.semantic

// Milestone 5c: ordinary method signatures. Everything a signature refers to
// is either nested in the class or a scala / java.lang class.

class Methods:
  trait Box:
    type Out

  def nullary: Int = 1
  def empty(): Int = 1
  def one(x: Int): Any = 1
  def curried(x: Int)(y: Any): Boolean = true
  def contextual[A](x: A)(using ord: Comparable[A]): A = x
  def polymorphic[A](x: A): A = x
  def fbounded[A <: Comparable[A]](x: A): A = x
  def dependent(x: Box): x.Out = ???
  def crossClause(x: Box)(y: x.Out): y.type = y
  def implicitClause(x: Int)(implicit y: Any): Int = 1
  def byNameParam(x: => Int): Int = x
  def typeOnly[A]: A = ???
  def repeated(xs: Int*): Int = 1
  def overloaded(x: Int): Int = 1
  def overloaded(x: Any): Any = x
