object ContextLambdaTemplateNextMember:
  val instance = new C:
    val value: Int | (Context ?=> Int) = ctx ?=> 1
    val after = 2
