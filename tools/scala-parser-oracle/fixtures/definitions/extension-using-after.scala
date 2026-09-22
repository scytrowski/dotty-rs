{
  extension (value: Value)(using ctx: Ctx)
    def render = ctx.render(value)
  result
}
