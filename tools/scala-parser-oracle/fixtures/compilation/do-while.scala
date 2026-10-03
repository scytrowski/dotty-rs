def repeat(ready: Boolean): Unit = do step() while ready

def preTest(ready: Boolean): Unit = while ready do step()
