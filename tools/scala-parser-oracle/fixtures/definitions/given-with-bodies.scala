{
  given Service with
    def run = result

  given Combined with Parent with
    def value = result

  given Extensions with
    extension (value: Int)
      def doubled = value * 2
}
