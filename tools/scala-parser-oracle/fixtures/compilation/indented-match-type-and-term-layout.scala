object IndentedMatchTypeAndTermLayout:
  type Flatten[X] = X match
    case List[t] => t match
      case Option[u] => u
      case _ => t
    case _ => X

  def nested(value: Option[Option[Int]]) =
    value match
      case Some(inner) =>
        inner match
          case Some(number) => number
          case None => 0
      case None => -1

  def after = 1
