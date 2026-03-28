package ConstraintsValidation {
    constraint def TimingConstraint {
        attribute maxLatency : Real;
    }
    part def Controller {
        constraint timing : TimingConstraint;
    }
}
