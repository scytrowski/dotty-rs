if condition then None
else {
  val reason = forwarderKind match {
    case 1 => TrivialMethod
    case 2 => FactoryMethod
    case 3 => BoxingForwarder
    case 4 => GenericForwarder
  }
  reason
}
