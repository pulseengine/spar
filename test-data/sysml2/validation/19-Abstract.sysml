// SysML v2 Validation: Abstract definitions
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package AbstractValidation {

    abstract part def Component {
        attribute id : Real;
    }

    abstract part def Sensor :> Component {
        attribute accuracy : Real;
    }

    abstract port def DataPort {
        out item data;
    }

    abstract action def Process {
        in item input;
        out item output;
    }

    part def ConcreteSensor :> Sensor {
        attribute accuracy = 0.01;
    }

    part mySensor : ConcreteSensor;
}
