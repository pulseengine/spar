// SysML v2 Training: Constraint Definitions
// Representative of Systems-Modeling/SysML-v2-Release sysml/src/training/08-*

package ConstraintDefinitionTraining {

    constraint def MassConstraint {
        doc "Total mass must not exceed limit"
    }

    constraint def PowerConstraint {
        doc "Power consumption must stay within budget"
    }

    constraint def LatencyConstraint {
        doc "End-to-end latency must be bounded"
    }

    part def System {
        constraint massLimit : MassConstraint;
        constraint powerBudget : PowerConstraint;
        constraint latencyBound : LatencyConstraint;
    }
}
