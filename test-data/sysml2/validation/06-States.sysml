// SysML v2 Validation: States
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package StatesValidation {

    state def VehicleStates {
        doc "Vehicle operational states"
    }

    state def EngineStates {
        doc "Engine operational states"
    }

    part def Vehicle {
        state vehicleState : VehicleStates;
    }

    part def Engine {
        state engineState : EngineStates;
    }
}
