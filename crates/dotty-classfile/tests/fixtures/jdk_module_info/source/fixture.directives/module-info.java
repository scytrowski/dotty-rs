module fixture.directives {
    requires static java.logging;

    exports fixture.api to fixture.consumer.one, fixture.consumer.two;
    opens fixture.impl to fixture.consumer.one;

    uses fixture.api.Greeting;
    provides fixture.api.Greeting with fixture.impl.GreetingProvider;
}
