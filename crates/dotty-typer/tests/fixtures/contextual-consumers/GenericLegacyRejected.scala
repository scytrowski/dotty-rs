package genericconversions

import scala.language.implicitConversions

class Bounded:
  implicit def bounded[A <: Child](x: Box[A]): Wrapped[A] = new Wrapped[A]
  def outsideBound(x: Box[Int]): Wrapped[Int] = x

class Conflict extends Client:
  def wrongResult(x: Box[Int]): Wrapped[Boolean] = x

class Ambiguous:
  implicit def first[A](x: Box[A]): Wrapped[A] = new Wrapped[A]
  implicit def second[A](x: Box[A]): Wrapped[A] = new Wrapped[A]
  def ambiguous(x: Box[Int]): Wrapped[Int] = x

class ImportMismatch:
  def wrongCompanionResult(x: Parcel[Int]): Wrapped[Boolean] = x
