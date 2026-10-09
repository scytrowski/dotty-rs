(
  e =>
    releaseThat(e).exit
      .flatMap(e1 =>
        releaseSelf(e).exit
          .flatMap(e2 => e1 *> e2)
      ),
  b
)
