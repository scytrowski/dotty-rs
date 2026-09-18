if outer then
  if inner then
    inner_value
  else
    alternative
  after_inner
else
  fallback
after_if

try
  risky()
catch
  case error =>
    recover()
finally
  cleanup()
after_try

while condition do
  if nested then
    work()
  after_work
after_while

for item <- items do
  item
yield item
after_for

given Service with
  if ready then
    service_value
  service_after
after_given
