object Outer:
  extension (self: Outer)
    def nested = 1
    end nested
  end extension
  def after = 2
end Outer
