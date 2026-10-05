object ColonLambdaBlockBody:
  val f = foo:
    x => x + 1
    val y = 2
    y
  val h = foo:
    x =>
      x + 1
    2
  val g = 3
