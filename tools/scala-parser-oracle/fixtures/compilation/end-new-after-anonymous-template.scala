object EndNewAfterAnonymousTemplate:
  def make(): Ordering[Int] =
    val label = "ordering"
    new Ordering[Int]:
      def compare(left: Int, right: Int): Int =
        val difference = left - right
        difference
      end compare
    end new
  end make
