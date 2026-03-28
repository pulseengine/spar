// SysML v2 Validation: Items
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package ItemsValidation {

    item def Fluid {
        attribute temperature : Real;
        attribute pressure : Real;
    }

    item def Signal {
        attribute voltage : Real;
        attribute frequency : Real;
    }

    item def DataPacket {
        attribute payload;
        attribute checksum;
    }

    part def Pipe {
        in item fluidIn : Fluid;
        out item fluidOut : Fluid;
    }

    part def Wire {
        in item signalIn : Signal;
        out item signalOut : Signal;
    }
}
