{
  type Simple = A @unchecked
  type Applied = List[A] @Ann
  type Repeated = A @Foo @Bar
  type AppliedAnnotation = A @Foo(1)
  type Union = A @Ann | B
  type Intersection = A & B @Ann
  type Grouped = (A | B) @Ann
}
