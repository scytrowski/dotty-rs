import language.experimental.captureChecking

def identity[T](value: T^{left, right}): T = value
