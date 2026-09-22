{
  given [A] => Show = makeShow
  given (using ctx: Ctx) => Service = makeService
  given () => Empty = makeEmpty
  value
}
