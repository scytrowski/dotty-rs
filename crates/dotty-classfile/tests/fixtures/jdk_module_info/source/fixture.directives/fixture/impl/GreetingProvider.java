package fixture.impl;

public final class GreetingProvider implements fixture.api.Greeting {
    public GreetingProvider() {}

    @Override
    public String greet() {
        return "hello";
    }
}
