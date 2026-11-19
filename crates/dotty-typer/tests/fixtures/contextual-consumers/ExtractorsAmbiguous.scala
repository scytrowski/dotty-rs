package contextualextractors

class Ambiguous:
  def choose(value: Int)(using one: Gate, two: Gate, mode: Mode): Int = value match
    case Allowed() => 1
    case _ => 0
