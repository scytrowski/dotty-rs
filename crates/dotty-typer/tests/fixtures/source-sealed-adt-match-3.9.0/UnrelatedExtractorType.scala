package adt

sealed trait Event

final case class Created(id: Int) extends Event
final case class Deleted(code: Int) extends Event

object Text:
  def unapply(value: Event): Option[String] = None

object Matcher:
  def extract(event: Event): Int =
    event match
      case Text(name)    => name.length
      case Deleted(code) => code

  def classify(event: Event): Int =
    event match
      case Created(0)  => 0
      case Created(id) => id
      case Deleted(_)  => 0
