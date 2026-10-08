def run = {
  val result = (for
    first <- sources
    second <- transform(first)
    _ <- discard(second)
  yield (first, second))
  after()
}
