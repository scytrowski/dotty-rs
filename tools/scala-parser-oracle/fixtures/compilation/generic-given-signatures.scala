object GenericGivenSignatures {
  protected given [DummySoItsADef]: Context = myContext

  given named[T]: Context[T] = makeContext[T]

  given canEqualSeqs[T, U](using eq: CanEqual[T, U]): CanEqual[Seq[T], Seq[U]] = derived

  given [T] => (using context: Context[T]) => Derived[T] = derive(context)

  given Ordering[Int]:
    def compare = 0
}
