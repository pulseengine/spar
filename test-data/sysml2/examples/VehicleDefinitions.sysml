// SysML v2 Example: Vehicle Definitions
// Based on Systems-Modeling/SysML-v2-Release sysml/src/examples/Vehicle Usage Example

package VehicleDefinitions {
    doc /* Definitions for vehicle structure modeling. */

    attribute def Torque;

    port def DriveIF {
        out item driveTorque : Torque;
    }

    port def MountingPoint;

    interface def Mounting {
        connect source.mount to target.hub;
    }

    part def Vehicle;

    part def Axle {
        port leftMountingPoint : MountingPoint;
        port rightMountingPoint : MountingPoint;
    }

    part def Wheel {
        port hub : MountingPoint;
    }

    part def Lugbolt {
        attribute tighteningTorque : Torque;
    }

    part def AxleAssembly {
        part axle : Axle;
    }

    part def Transmission {
        port drive : DriveIF;
    }
}
