object NestedCaseLambda:
  val f: Int => (Int => Int) =
    case 0 =>
      case 1 => 1
      case x => x
    case n =>
      case x => n + x
  val g = 2
