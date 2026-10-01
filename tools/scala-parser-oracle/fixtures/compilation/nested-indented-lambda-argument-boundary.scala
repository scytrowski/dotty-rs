object NestedIndentedLambdaArgumentBoundary:
  def check(input: Boolean): Option[Int] =
    input match {
      case true =>
        Some(1).flatMap(value =>
          val nested = value
          if nested > 0 then
            val result = nested
            if result > 1 then Some(result) else None
          else None
        )
      case false => None
    }
