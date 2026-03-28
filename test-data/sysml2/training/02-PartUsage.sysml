// SysML v2 Training: Part Usages
// Representative of Systems-Modeling/SysML-v2-Release sysml/src/training/02-*

package PartUsageTraining {

    part def Vehicle;
    part def Wheel;
    part def Engine;

    part vehicle1 : Vehicle {
        part frontWheels [2] : Wheel;
        part rearWheels [2] : Wheel;
        part engine : Engine;
    }

    part vehicle2 : Vehicle {
        part wheels [4] : Wheel;
        part engine : Engine;
    }
}
