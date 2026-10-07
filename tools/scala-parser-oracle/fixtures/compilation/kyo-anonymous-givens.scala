object KyoAnonymousGivens:
  inline given [A](using inline evidence: Evidence[A]): CanEqual[Box[A], Box[A]] = CanEqual.derived

  given [A](using render: Render[A]): Render[Box[A]] with
    def render(value: Box[A]): String = render.asString(value.value)

  given [A]: Box[A] = Box.empty[A]

  given (using ctx: Ctx): Service = makeService
