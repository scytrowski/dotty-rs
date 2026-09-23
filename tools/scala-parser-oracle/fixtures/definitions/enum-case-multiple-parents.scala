{
  enum Expr:
    case Lit(value: Int) extends Node(value), Marker
}
