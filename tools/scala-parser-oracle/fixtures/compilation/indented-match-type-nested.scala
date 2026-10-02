type Flatten[X] = X match
  case List[t] => t match
    case Option[u] => u
    case _ => t
  case _ => X
