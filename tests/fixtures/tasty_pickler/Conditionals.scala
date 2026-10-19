package example

class Conditionals:
  def choose(flag: Boolean, a: Int, b: Int): Int =
    if flag then a else b
  def nested(a: Boolean, b: Boolean): Int =
    if a then
      if b then 1 else 2
    else 3
  def constant: Int = if true then 17 else 23
  def mixed(flag: Boolean, value: Int): Int = if flag then value else 42
  def identity(value: Int): Int = value
  def callBranch(flag: Boolean, value: Int): Int =
    if flag then identity(value) else 42
