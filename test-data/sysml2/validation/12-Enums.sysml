// SysML v2 Validation: Enumerations
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package EnumsValidation {

    enum def Color {
        doc "Primary colors enumeration"
    }

    enum def Priority {
        doc "Task priority levels"
    }

    enum def OperatingMode {
        doc "System operating modes"
    }

    part def Display {
        attribute backgroundColor : Color;
    }

    part def Task {
        attribute priority : Priority;
    }
}
