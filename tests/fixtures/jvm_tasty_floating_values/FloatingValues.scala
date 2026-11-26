package example
class FloatingValues {
 def fzero(): Float = 0.0f
 def fnegativeZero(): Float = -0.0f
 def fsmall(): Float = 1.4e-45f
 def flarge(): Float = 3.4028235e38f
 def dzero(): Double = 0.0
 def dnegativeZero(): Double = -0.0
 def dsmall(): Double = 4.9e-324
 def dlarge(): Double = 1.7976931348623157e308
 def directF(x: Float): Float = x
 def directD(x: Double): Double = x
 def savedF(x: Float): Float = { val saved = x; saved }
 def savedD(x: Double): Double = { var saved: Double = x; saved = x; saved }
 def chooseF(flag: Boolean, a: Float, b: Float): Float = if flag then a else b
 def chooseD(flag: Boolean, a: Double, b: Double): Double = if flag then a else b
 def rotateF(n: Int, a: Float, b: Float): Float = { var saved: Float = a; var i: Int = n; while i > 0 do { saved = b; i = i - 1 }; saved }
 def rotateD(n: Int, a: Double, b: Double): Double = { var saved: Double = a; var i: Int = n; while i > 0 do { saved = b; i = i - 1 }; saved }
 def returnedF(flag: Boolean, a: Float, b: Float): Float = if flag then return a else return b
 def returnedD(flag: Boolean, a: Double, b: Double): Double = if flag then return a else b
 def allReturnF(x: Int, a: Float, b: Float): Float = x match { case 0 | 1 => return a; case _ => return b }
 def allReturnD(x: Int, a: Double, b: Double): Double = x match { case 0 | 1 => return a; case _ => return b }
 def mixed(i: Int, a: Float, n: Long, b: Double, flag: Boolean): Double = if flag then b else -0.0
 def matchedF(x: Int, flag: Boolean, a: Float, b: Float): Float = x match { case 0 | 1 if flag => a; case _ => b }
 def matchedD(x: Int, flag: Boolean, a: Double, b: Double): Double = x match { case 0 | 1 if flag => a; case _ => b }
 def deadF(flag: Boolean, a: Float, b: Float): Float = if flag then { return a; b } else b
 def deadD(flag: Boolean, a: Double, b: Double): Double = if flag then { val saved: Double = b; return a; saved } else b
 private var trace: Int = 0
 def next(x: Int): Int = { trace = trace * 10 + 1; x }
 def guardCode(): Int = { trace = trace * 10 + 2; 0 }
 def observed(): Int = trace
 def guardedF(x: Int, a: Float, b: Float): Float = next(x) match { case 0 | 0 if guardCode() > 0 => a; case _ => b }
 def boundD(x: Int, a: Double, b: Double): Double = x match { case n if n > 0 => a; case _ => b }
 def inLoopD(n: Int, a: Double, b: Double): Double = { var i: Int = n; while i > 0 do { if i == 2 then return a; i = i - 1 }; b }
}
