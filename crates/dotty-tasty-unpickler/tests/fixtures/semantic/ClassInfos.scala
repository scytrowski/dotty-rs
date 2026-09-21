package me.cytrowski.tastyfixtures.semantic

// Milestone 5d1: class completion. Parents, an explicit self type and nested
// classes, with members that class completion must leave alone.

trait InfoBase[A]

trait InfoDep

class InfoParent[A]

class InfoChild[A](val value: A) extends InfoParent[A] with InfoBase[A]:
  self: InfoDep =>
  type Member = A
  def method(x: A): A = x

object InfoHolder:
  class Nested extends InfoParent[Int]
  object Inner:
    val deep: Int = 0

trait InfoNeeds:
  self: InfoDep =>

class InfoWrapped[F[_]](x: Int) extends InfoBase[F[Int]]:
  def m: F[Int] = ???

trait InfoLam[F[_]]

class InfoLambdaParent extends InfoLam[[X] =>> InfoBase[X]]

trait InfoLambdaSelf:
  self: InfoLam[[X] =>> InfoBase[X]] =>

class InfoGenSelf[A]:
  self: InfoBase[A] =>

trait InfoNeedsGeneric[A]:
  self: InfoDep =>
