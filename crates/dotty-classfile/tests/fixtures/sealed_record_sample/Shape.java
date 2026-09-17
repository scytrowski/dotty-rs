public sealed interface Shape permits Shape.Circle, Shape.Square {
    record Circle(double radius) implements Shape {
    }

    record Square(double side) implements Shape {
    }
}
