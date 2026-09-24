{
  type Member = T#Member
  type Nested = Outer#Inner
  type Applied = F[A]#Result
  type Repeated = F[A]#Outer#Inner
  type Dotted = pkg.Outer#Inner
}
