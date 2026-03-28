// SysML v2 Validation: Calculations
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package CalcsValidation {

    calc def TotalMass {
        doc "Calculate total mass of a system"
    }

    calc def MaxSpeed {
        doc "Calculate maximum achievable speed"
    }

    calc def FuelConsumption {
        doc "Calculate fuel consumption rate"
    }

    part def Vehicle {
        attribute mass : Real;
        attribute speed : Real;
    }
}
