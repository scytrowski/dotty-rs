package example
class BooleanOperators {
def and_formals(a: Boolean,b: Boolean): Boolean = a & b
def and_literals(a: Boolean,b: Boolean): Boolean = true & false
def and_nested(a: Boolean,b: Boolean): Boolean = !(a & b) || (a && b)
def and_operands(a: Boolean,b: Boolean): Boolean = ({ var n: Int = 0; n = n + 1; a } & { var n: Int = 0; n = n + 1; b })
def and_local(a: Boolean,b: Boolean): Boolean = {val result = a & b; result}
def and_match(a: Boolean,b: Boolean): Boolean = (a & b) match {case true => true;case false => false}
def and_if(a: Boolean,b: Boolean): Boolean = if a & b then true else false
def and_guard(a: Boolean,b: Boolean): Int = a match {case n if n & b => 1;case _ => 2}
def and_while(a: Boolean,b: Boolean): Int = {var n: Int = 0; while n < 2 && (a & b) do n = n + 1; n}
def and_discard(a: Boolean,b: Boolean): Unit = a & b
def and_return(a: Boolean,b: Boolean): Boolean = {if a then return a & b; b}
def and_order(a: Boolean,b: Boolean): Int = {var order: Int = 0; val value = ({order = order * 10 + 1; a} & {order = order * 10 + 2; b}); order}
def and_discard_order(a: Boolean,b: Boolean): Int = {var order: Int = 0; ({order = order * 10 + 1; a} & {order = order * 10 + 2; b}); order}
def and_throw(a: Boolean,x: Int): Boolean = a & (10 / x > 0)
def and_after_return(a: Boolean,b: Boolean,x: Int): Int = {if a then return 3; b & (10 / x > 0); 7}
def or_formals(a: Boolean,b: Boolean): Boolean = a | b
def or_literals(a: Boolean,b: Boolean): Boolean = true | false
def or_nested(a: Boolean,b: Boolean): Boolean = !(a | b) || (a && b)
def or_operands(a: Boolean,b: Boolean): Boolean = ({ var n: Int = 0; n = n + 1; a } | { var n: Int = 0; n = n + 1; b })
def or_local(a: Boolean,b: Boolean): Boolean = {val result = a | b; result}
def or_match(a: Boolean,b: Boolean): Boolean = (a | b) match {case true => true;case false => false}
def or_if(a: Boolean,b: Boolean): Boolean = if a | b then true else false
def or_guard(a: Boolean,b: Boolean): Int = a match {case n if n | b => 1;case _ => 2}
def or_while(a: Boolean,b: Boolean): Int = {var n: Int = 0; while n < 2 && (a | b) do n = n + 1; n}
def or_discard(a: Boolean,b: Boolean): Unit = a | b
def or_return(a: Boolean,b: Boolean): Boolean = {if a then return a | b; b}
def or_order(a: Boolean,b: Boolean): Int = {var order: Int = 0; val value = ({order = order * 10 + 1; a} | {order = order * 10 + 2; b}); order}
def or_discard_order(a: Boolean,b: Boolean): Int = {var order: Int = 0; ({order = order * 10 + 1; a} | {order = order * 10 + 2; b}); order}
def or_throw(a: Boolean,x: Int): Boolean = a | (10 / x > 0)
def or_after_return(a: Boolean,b: Boolean,x: Int): Int = {if a then return 3; b | (10 / x > 0); 7}
def xor_formals(a: Boolean,b: Boolean): Boolean = a ^ b
def xor_literals(a: Boolean,b: Boolean): Boolean = true ^ false
def xor_nested(a: Boolean,b: Boolean): Boolean = !(a ^ b) || (a && b)
def xor_operands(a: Boolean,b: Boolean): Boolean = ({ var n: Int = 0; n = n + 1; a } ^ { var n: Int = 0; n = n + 1; b })
def xor_local(a: Boolean,b: Boolean): Boolean = {val result = a ^ b; result}
def xor_match(a: Boolean,b: Boolean): Boolean = (a ^ b) match {case true => true;case false => false}
def xor_if(a: Boolean,b: Boolean): Boolean = if a ^ b then true else false
def xor_guard(a: Boolean,b: Boolean): Int = a match {case n if n ^ b => 1;case _ => 2}
def xor_while(a: Boolean,b: Boolean): Int = {var n: Int = 0; while n < 2 && (a ^ b) do n = n + 1; n}
def xor_discard(a: Boolean,b: Boolean): Unit = a ^ b
def xor_return(a: Boolean,b: Boolean): Boolean = {if a then return a ^ b; b}
def xor_order(a: Boolean,b: Boolean): Int = {var order: Int = 0; val value = ({order = order * 10 + 1; a} ^ {order = order * 10 + 2; b}); order}
def xor_discard_order(a: Boolean,b: Boolean): Int = {var order: Int = 0; ({order = order * 10 + 1; a} ^ {order = order * 10 + 2; b}); order}
def xor_throw(a: Boolean,x: Int): Boolean = a ^ (10 / x > 0)
def xor_after_return(a: Boolean,b: Boolean,x: Int): Int = {if a then return 3; b ^ (10 / x > 0); 7}
def eq_formals(a: Boolean,b: Boolean): Boolean = a == b
def eq_literals(a: Boolean,b: Boolean): Boolean = true == false
def eq_nested(a: Boolean,b: Boolean): Boolean = !(a == b) || (a && b)
def eq_operands(a: Boolean,b: Boolean): Boolean = ({ var n: Int = 0; n = n + 1; a } == { var n: Int = 0; n = n + 1; b })
def eq_local(a: Boolean,b: Boolean): Boolean = {val result = a == b; result}
def eq_match(a: Boolean,b: Boolean): Boolean = (a == b) match {case true => true;case false => false}
def eq_if(a: Boolean,b: Boolean): Boolean = if a == b then true else false
def eq_guard(a: Boolean,b: Boolean): Int = a match {case n if n == b => 1;case _ => 2}
def eq_while(a: Boolean,b: Boolean): Int = {var n: Int = 0; while n < 2 && (a == b) do n = n + 1; n}
def eq_discard(a: Boolean,b: Boolean): Unit = a == b
def eq_return(a: Boolean,b: Boolean): Boolean = {if a then return a == b; b}
def eq_order(a: Boolean,b: Boolean): Int = {var order: Int = 0; val value = ({order = order * 10 + 1; a} == {order = order * 10 + 2; b}); order}
def eq_discard_order(a: Boolean,b: Boolean): Int = {var order: Int = 0; ({order = order * 10 + 1; a} == {order = order * 10 + 2; b}); order}
def eq_throw(a: Boolean,x: Int): Boolean = a == (10 / x > 0)
def eq_after_return(a: Boolean,b: Boolean,x: Int): Int = {if a then return 3; b == (10 / x > 0); 7}
def ne_formals(a: Boolean,b: Boolean): Boolean = a != b
def ne_literals(a: Boolean,b: Boolean): Boolean = true != false
def ne_nested(a: Boolean,b: Boolean): Boolean = !(a != b) || (a && b)
def ne_operands(a: Boolean,b: Boolean): Boolean = ({ var n: Int = 0; n = n + 1; a } != { var n: Int = 0; n = n + 1; b })
def ne_local(a: Boolean,b: Boolean): Boolean = {val result = a != b; result}
def ne_match(a: Boolean,b: Boolean): Boolean = (a != b) match {case true => true;case false => false}
def ne_if(a: Boolean,b: Boolean): Boolean = if a != b then true else false
def ne_guard(a: Boolean,b: Boolean): Int = a match {case n if n != b => 1;case _ => 2}
def ne_while(a: Boolean,b: Boolean): Int = {var n: Int = 0; while n < 2 && (a != b) do n = n + 1; n}
def ne_discard(a: Boolean,b: Boolean): Unit = a != b
def ne_return(a: Boolean,b: Boolean): Boolean = {if a then return a != b; b}
def ne_order(a: Boolean,b: Boolean): Int = {var order: Int = 0; val value = ({order = order * 10 + 1; a} != {order = order * 10 + 2; b}); order}
def ne_discard_order(a: Boolean,b: Boolean): Int = {var order: Int = 0; ({order = order * 10 + 1; a} != {order = order * 10 + 2; b}); order}
def ne_throw(a: Boolean,x: Int): Boolean = a != (10 / x > 0)
def ne_after_return(a: Boolean,b: Boolean,x: Int): Int = {if a then return 3; b != (10 / x > 0); 7}
def short_and_formals(a: Boolean,b: Boolean): Boolean = a && b
def short_and_literals(a: Boolean,b: Boolean): Boolean = true && false
def short_and_nested(a: Boolean,b: Boolean): Boolean = !(a && b) || (a && b)
def short_and_operands(a: Boolean,b: Boolean): Boolean = ({ var n: Int = 0; n = n + 1; a } && { var n: Int = 0; n = n + 1; b })
def short_and_local(a: Boolean,b: Boolean): Boolean = {val result = a && b; result}
def short_and_match(a: Boolean,b: Boolean): Boolean = (a && b) match {case true => true;case false => false}
def short_and_if(a: Boolean,b: Boolean): Boolean = if a && b then true else false
def short_and_guard(a: Boolean,b: Boolean): Int = a match {case n if n && b => 1;case _ => 2}
def short_and_while(a: Boolean,b: Boolean): Int = {var n: Int = 0; while n < 2 && (a && b) do n = n + 1; n}
def short_and_discard(a: Boolean,b: Boolean): Unit = a && b
def short_and_return(a: Boolean,b: Boolean): Boolean = {if a then return a && b; b}
def short_and_order(a: Boolean,b: Boolean): Int = {var order: Int = 0; val value = ({order = order * 10 + 1; a} && {order = order * 10 + 2; b}); order}
def short_and_discard_order(a: Boolean,b: Boolean): Int = {var order: Int = 0; ({order = order * 10 + 1; a} && {order = order * 10 + 2; b}); order}
def short_and_throw(a: Boolean,x: Int): Boolean = a && (10 / x > 0)
def short_and_after_return(a: Boolean,b: Boolean,x: Int): Int = {if a then return 3; b && (10 / x > 0); 7}
def short_or_formals(a: Boolean,b: Boolean): Boolean = a || b
def short_or_literals(a: Boolean,b: Boolean): Boolean = true || false
def short_or_nested(a: Boolean,b: Boolean): Boolean = !(a || b) || (a && b)
def short_or_operands(a: Boolean,b: Boolean): Boolean = ({ var n: Int = 0; n = n + 1; a } || { var n: Int = 0; n = n + 1; b })
def short_or_local(a: Boolean,b: Boolean): Boolean = {val result = a || b; result}
def short_or_match(a: Boolean,b: Boolean): Boolean = (a || b) match {case true => true;case false => false}
def short_or_if(a: Boolean,b: Boolean): Boolean = if a || b then true else false
def short_or_guard(a: Boolean,b: Boolean): Int = a match {case n if n || b => 1;case _ => 2}
def short_or_while(a: Boolean,b: Boolean): Int = {var n: Int = 0; while n < 2 && (a || b) do n = n + 1; n}
def short_or_discard(a: Boolean,b: Boolean): Unit = a || b
def short_or_return(a: Boolean,b: Boolean): Boolean = {if a then return a || b; b}
def short_or_order(a: Boolean,b: Boolean): Int = {var order: Int = 0; val value = ({order = order * 10 + 1; a} || {order = order * 10 + 2; b}); order}
def short_or_discard_order(a: Boolean,b: Boolean): Int = {var order: Int = 0; ({order = order * 10 + 1; a} || {order = order * 10 + 2; b}); order}
def short_or_throw(a: Boolean,x: Int): Boolean = a || (10 / x > 0)
def short_or_after_return(a: Boolean,b: Boolean,x: Int): Int = {if a then return 3; b || (10 / x > 0); 7}
}
