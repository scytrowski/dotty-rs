def f(x: Int) =
  x match
    case 0 =>
      val y =
        x match
          case 0 => 1
          case _ => 2
      { y }
    case _ => 3
