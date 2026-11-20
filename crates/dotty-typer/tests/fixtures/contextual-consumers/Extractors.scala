package contextualextractors

trait Gate
trait Mode
object Allowed:
  def unapply(value: Int)(using gate: Gate)(using mode: Mode): Boolean = true

class Reader(using fallback: Gate):
  def choose(value: Int)(using inner: Gate, mode: Mode): Int = value match
    case Allowed() => 1
    case _ => 0
