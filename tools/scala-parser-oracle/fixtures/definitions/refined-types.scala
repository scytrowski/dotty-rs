{
  type Abstract = A { type X }
  type Alias = A { type X = String }
  type Bounded = A { type X <: Upper }
  type Multiple = A {
    type X
    type Y = Int
  }
  type Parentless = { type X }
  type Annotated = A @unchecked { type X }
  type Applied = List[A] { type X }
}
