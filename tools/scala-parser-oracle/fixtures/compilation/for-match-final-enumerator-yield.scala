package repro

object ForMatchFinalEnumeratorYield:
  def values(routes: List[String]) =
    Loop.forever {
      for
        nextRoute <- routes
        segments = nextRoute.split('/').filter(_.nonEmpty)
        _ <- classifyRoute(segments) match
          case 0 =>
            List(segments)
          case _ =>
            Nil
      yield Loop.continue
    }
