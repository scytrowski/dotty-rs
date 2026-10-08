object FunctionTypeProjection:
  class A
  class B
  class C

  type Zero = () => A
  type One = A => B
  type Many = (A, B) => C
  type NestedResult = A => B => C
  type FunctionParameter = (A => B) => C
  type Contextual = (x: A) ?=> B
