package repro

object NewTemplateFollowedByTuple:
  def make =
    Callback.init.map { recorded =>
      val service =
        new Service:
          def connect =
            1
          end connect
      (service, recorded)
    }
