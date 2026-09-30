import language.experimental.captureChecking

trait CaptureSetTypes:
  this: ofBoolean^{} =>

  type View = MapView[Key, Value]^{this}
