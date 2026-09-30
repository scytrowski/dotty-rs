object NestedEndIf:
  def run = {
    if phaseWillRun then
      trackTime {
        work()
      }
      if phasesWereAdjusted then
        if !captureCheckingEnabled then
          unlinkCapturePhase()
        end if
      end if
    end if
  }
end NestedEndIf
