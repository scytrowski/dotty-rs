{
  object UsingPrefixedNamedParameters:
    def annotated(using @constructorOnly ctx: Context): Unit = ()
    class Constructor(using val ctx: Context)
}
