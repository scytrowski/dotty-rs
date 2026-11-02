package adt

sealed trait Event

final case class Created(id: Int) extends Event
final case class Deleted(code: Int) extends Event

object Matcher:
  def extract(event: Event): Int =
    event match
      case Created(id)   => id
      case Deleted(code) => code

  def classify(event: Event): Int =
    event match
      case Created(0)  => 0
      case Created(id) => id
      case Created(other) => other
      case Deleted(_)  => 0
