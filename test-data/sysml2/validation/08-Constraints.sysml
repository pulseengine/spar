// SysML v2 Validation: Constraints
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package ConstraintsValidation {

    constraint def TimingConstraint {
        doc "Timing constraint for real-time systems"
    }

    constraint def PowerConstraint {
        doc "Power budget constraint"
    }

    constraint def ThermalConstraint {
        doc "Thermal envelope constraint"
    }

    part def EmbeddedSystem {
        constraint timing : TimingConstraint;
        constraint power : PowerConstraint;
        constraint thermal : ThermalConstraint;
    }
}
