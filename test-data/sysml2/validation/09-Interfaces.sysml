// SysML v2 Validation: Interfaces
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package InterfacesValidation {

    port def CommandPort {
        out item command;
    }

    port def StatusPort {
        in item status;
    }

    interface def ControlInterface {
        connect source.command to target.status;
    }

    part def Master {
        port cmdOut : CommandPort;
    }

    part def Slave {
        port statusIn : StatusPort;
    }

    part def System {
        part master : Master;
        part slave : Slave;
        interface controlLink : ControlInterface;
    }
}
