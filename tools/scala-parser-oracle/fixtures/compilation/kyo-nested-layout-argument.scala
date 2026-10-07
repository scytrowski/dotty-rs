object KyoLayout:
    def map =
        Loop.handle(
            [C] =>
                (input, cont) =>
                    val next = input.map(identity)
                    if next.isEmpty then Loop.continue(cont(()))
                    else Emit.valueWith(next)(Loop.continue(cont(())))
        )
end KyoLayout
