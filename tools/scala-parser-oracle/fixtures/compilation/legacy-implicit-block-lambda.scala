def fromFuture = flatMap(executionContext) {
  implicit ec: ExecutionContext & Executor =>
    val result = 1
    use(ec, result)
}

def fromFutureUntyped = flatMap(executionContext) { implicit ec => use(ec) }
