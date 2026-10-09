object Outer:
    object Dynamic:
        sealed abstract class Unsafe:
            def nested =
                if true then
                    1
                end if
            end nested
        end Unsafe
        object Unsafe:
            val instance: Unsafe = new Unsafe {}
        end Unsafe
        def after = 2
    end Dynamic
end Outer
