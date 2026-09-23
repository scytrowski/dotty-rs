{
  val f: (ctx: Context) ?=> Result = value
  def run(f: (ctx: Context, req: Request) ?=> Response): Result = body
  class Box(callback: (ctx: Context) ?=> Result)
  given Ordering[(ctx: Context) ?=> Result] = ordering
  f
}
