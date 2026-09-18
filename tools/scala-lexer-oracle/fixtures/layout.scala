if ready then
  run()
finish()

value
  + other

value

  + other

value
  `op` other

value

  `op` other

if condition then
  first()
else
  second()
after()

try
  risky()
catch
  case error => recover()
finally
  cleanup()
after_try()

value match
  case first =>
    one()
  case second =>
    two()
after_match()

do
  body()
while condition
after_do()

value match
  case outer =>
    value match
      case inner =>
        inner()
    after_inner()
  case next =>
    next()
after_nested_match()

for item <- items do
  item
yield item
after_for()

given Service with
  service_value
after_given()

while condition do
  work()
after_while()
