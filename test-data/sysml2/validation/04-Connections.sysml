// SysML v2 Validation: Connections
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package ConnectionsValidation {

    port def DataPort {
        out item data;
    }

    port def ReceiverPort {
        in item data;
    }

    connection def DataLink {
        connect source.data to target.data;
    }

    part def Sender {
        port outPort : DataPort;
    }

    part def Receiver {
        port inPort : ReceiverPort;
    }

    part def System {
        part sender : Sender;
        part receiver : Receiver;
        connect sender.outPort to receiver.inPort;
    }
}
