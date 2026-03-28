// SysML v2 Validation: Parts
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package PartsValidation {

    part def Vehicle {
        attribute mass : Real;
        attribute maxSpeed : Real;
    }

    part def Engine {
        attribute power : Real;
        attribute displacement : Real;
    }

    part def Transmission {
        attribute gearRatio : Real;
    }

    part def Wheel {
        attribute diameter : Real;
    }

    part myVehicle : Vehicle {
        part engine : Engine {
            attribute power = 150;
        }
        part transmission : Transmission;
        part frontWheels [2] : Wheel;
        part rearWheels [2] : Wheel;
    }
}
