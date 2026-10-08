(
  for
    _ <- tick
    fiber <- push
    _ <- pull
    _ <- fiber.get
  yield ()
)
