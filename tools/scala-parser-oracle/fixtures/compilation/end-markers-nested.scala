object SetupAPI:
  def configure =
    if ready then
      run()
    end if
  end configure
end SetupAPI

object After
