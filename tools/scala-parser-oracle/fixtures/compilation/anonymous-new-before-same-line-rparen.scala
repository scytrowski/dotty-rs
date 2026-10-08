object Outer:
  def run =
    consume(new Base:
      def close = ())
  def after = 1
end Outer
