object NestedMatchMultilineGuard:
  def select(input: Option[Int], values: List[Int], opt: Int) = {
    input match
      case Some(value) =>
        val selected = value match
          case 0 => 1
          case other => other
        values match
          case _ =>
            val (first, rest) = input match
              case Some(items) if opt == 0
                                  && values.size == 1
                                  && selected > 0 =>
                (Nil, items)
              case _ =>
                val fallback = selected match
                  case 0 => None
                  case _ => Some(selected)
                (List(fallback), value)
              val useSelectors = values.length <= 22
              def selector(index: Int) = rest match
                case items: List[Int] => items(index)
                case item => item
              selector(0)
      case None => 0
  }
