object AnonymousTemplateArguments:
  def pair = consume(
    new First:
      def first = 1,
    new Second:
      def second = 2
  )
