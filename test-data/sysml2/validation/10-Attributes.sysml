// SysML v2 Validation: Attributes
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package AttributesValidation {

    attribute def Mass;
    attribute def Speed;
    attribute def Length;
    attribute def Temperature;

    part def Vehicle {
        attribute mass : Mass;
        attribute maxSpeed : Speed = 200;
        attribute length : Length;
    }

    part def Sensor {
        attribute range : Length;
        attribute accuracy : Real = 0.01;
        attribute operatingTemp : Temperature;
    }

    part def System {
        part vehicle : Vehicle {
            attribute mass = 1500;
            attribute length = 4500;
        }
        part sensor : Sensor {
            attribute range = 100;
        }
    }
}
