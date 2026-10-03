value match {
  case
    // The first pattern alternative is kept on its own line.
    _: WithLazyFields

    // A symbol is created before the corresponding tree is unpickled.
    // Its initial span is not sufficient to reconstruct the position.
    | _: Trees.DefTree[?]

    // Package definitions can be split across TASTy files.
    | _: Trees.PackageDef[?]
    // Holes can change source files when filled.
    | _: Trees.Hole[?] => true
}
