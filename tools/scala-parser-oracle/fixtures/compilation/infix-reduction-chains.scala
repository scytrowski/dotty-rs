val result = a +: b * c + d op e :: f :: g
type Result = A +: B * C + D op E :: F :: G
val matched = input match { case a +: b * c + d op e :: f :: g => a }
