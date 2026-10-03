def id(info: Int) = info

def f(symd: Any) =
  id(info = symd.name match
    case A => 1
    case n if n == x || n == y =>
      2)

def after = 2
