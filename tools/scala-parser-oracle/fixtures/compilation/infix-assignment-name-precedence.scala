val result = a += b :: c
type Result = A += B :: C
type Incoming = A :: B += C
val matched = input match { case a += b :: c => a }
