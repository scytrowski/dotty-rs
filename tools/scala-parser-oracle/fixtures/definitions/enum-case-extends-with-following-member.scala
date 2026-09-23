{
  enum Expr:
    case Lit(value: Int) extends Node
    def after = 1
}
