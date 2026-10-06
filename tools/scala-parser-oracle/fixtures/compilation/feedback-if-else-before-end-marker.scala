object O:
  def f =
    object Inner {
      def g =
        if cond then
          yes
        else no
    }
    1
  end f
