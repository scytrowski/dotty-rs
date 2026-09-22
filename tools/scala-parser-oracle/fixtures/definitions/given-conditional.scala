{
  given [A] => Show = makeShow
  given (using ctx: Ctx) => Service = makeService
  given (ctx: Ctx) => PlainService = makePlainService
  given () => Empty = makeEmpty
  value
}
