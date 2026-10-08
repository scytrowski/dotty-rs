object O:
  def run =
    try
      work()
    finally
      cleanup()
    end try
  def after = true
