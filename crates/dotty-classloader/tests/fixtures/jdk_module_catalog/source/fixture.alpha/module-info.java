module fixture.alpha {
    requires transitive fixture.beta;
    exports fixture.alpha.api to fixture.beta;
    opens fixture.alpha.internal;
    uses fixture.alpha.api.Service;
    provides fixture.alpha.api.Service with fixture.alpha.impl.Provider;
}
