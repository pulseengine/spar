// SysML v2 Training: State Definitions
// Representative of Systems-Modeling/SysML-v2-Release sysml/src/training/06-*

package StateDefinitionTraining {

    state def VehicleStates {
        doc "Operational states of a vehicle"
    }

    state def EngineStates {
        doc "Engine lifecycle states"
    }

    state def BrakeStates {
        doc "Brake system states"
    }

    part def Vehicle {
        state operational : VehicleStates;
        part engine {
            state lifecycle : EngineStates;
        }
    }
}
