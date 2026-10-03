package p {
  object Extractor {
    def unapply(value: Int): Boolean = true
  }
}

package client {
  object PackageSelectedExtractor {
    def matches(value: Int): Boolean = value match {
      case p.Extractor() => true
      case _ => false
    }
  }
}
