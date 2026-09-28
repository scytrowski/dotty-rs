value match {
  case A =>
    val local = 1
    def identity = local
    identity
  case B => 0
}
