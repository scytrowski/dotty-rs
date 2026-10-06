def f(p: Boolean): Int = 0 match
  case 0 => if p then 1 else 2
  case 1 => 3
