import language.experimental.captureChecking

type InfixCaret = A^B
type PureFunction = A -> {cap} B
type PureContextFunction = A ?-> {cap} B
type EmptyPureFunction = A -> {} B
type ImpureFunction = A => B
