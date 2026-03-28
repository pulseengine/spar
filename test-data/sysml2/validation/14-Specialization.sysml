// SysML v2 Validation: Specialization and Typing
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package SpecializationValidation {

    part def Vehicle {
        attribute mass : Real;
    }

    part def Car specializes Vehicle {
        attribute numDoors : Real;
    }

    part def Truck specializes Vehicle {
        attribute payloadCapacity : Real;
    }

    part def ElectricCar :> Car {
        attribute batteryCapacity : Real;
    }

    part myCar : Car;
    part myTruck : Truck;
    part myEV :> ElectricCar;

    // Redefines
    part def CustomVehicle {
        part engine redefines baseEngine;
    }

    // Subsets
    part def Fleet {
        part vehicles [0..*] : Vehicle;
        part trucks [0..*] subsets vehicles;
    }

    // Colon-greater-greater redefines
    part shorthandRedefine :>> myCar;
}
