// SysML v2 Validation: Multiplicity
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package MultiplicityValidation {

    part def Wheel;
    part def Lugbolt;
    part def Passenger;

    part def Vehicle {
        part wheels [4] : Wheel;
        part passengers [0..5] : Passenger;
        part spareWheel [0..1] : Wheel;
    }

    part def WheelAssembly {
        part lugbolts [4..6] : Lugbolt;
    }

    part def Fleet {
        part vehicles [0..*] : Vehicle;
    }

    part def Convoy {
        part vehicles [1..*] : Vehicle;
    }
}
