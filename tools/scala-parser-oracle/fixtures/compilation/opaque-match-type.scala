opaque type Elem[X] <: Any = X match {
  case List[t] => t
  case _ => X
}
