{
  object ByNameParameterProbe:
    def use(action: => Unit, value: => pkg.Type[Arg]): Unit = ()
}
