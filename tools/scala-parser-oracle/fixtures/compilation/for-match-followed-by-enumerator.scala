package repro

object ForMatchFollowedByEnumerator:
  def values(routes: List[List[Int]]) =
    for
      value <- classifyRoute(0) match
        case 0 =>
          List(1)
        case _ =>
          Nil
      next <- routes
    yield next
