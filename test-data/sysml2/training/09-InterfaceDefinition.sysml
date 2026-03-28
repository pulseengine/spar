// SysML v2 Training: Interface Definitions
// Representative of Systems-Modeling/SysML-v2-Release sysml/src/training/09-*

package InterfaceDefinitionTraining {

    port def DataPort {
        out item data;
    }

    port def ControlPort {
        in item command;
        out item status;
    }

    interface def DataBus {
        doc "High-speed data bus interface"
    }

    interface def ControlBus {
        doc "Control and status bus interface"
    }

    part def Subsystem {
        port dataOut : DataPort;
        port controlIn : ControlPort;
    }

    part def MainComputer {
        port dataIn;
        port controlOut;
        interface dataBus : DataBus;
    }
}
