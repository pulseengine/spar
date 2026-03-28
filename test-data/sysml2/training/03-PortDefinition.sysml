// SysML v2 Training: Port Definitions
// Representative of Systems-Modeling/SysML-v2-Release sysml/src/training/03-*

package PortDefinitionTraining {

    attribute def Force;
    attribute def Torque;

    port def FuelPort {
        in item fuelIn;
        out item exhaustOut;
    }

    port def ElectricalPort {
        inout item power;
    }

    port def MechanicalPort {
        out item torque : Torque;
        out item force : Force;
    }

    part def Engine {
        port fuel : FuelPort;
        port electrical : ElectricalPort;
        port output : MechanicalPort;
    }
}
