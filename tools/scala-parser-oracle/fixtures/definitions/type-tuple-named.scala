{
  type User = (name: String, age: Int)
  type Nested = (items: List[String | Int], callback: A => B)
  type Plain = (A, B)
  type Contextual = (ctx: C) ?=> R
}
