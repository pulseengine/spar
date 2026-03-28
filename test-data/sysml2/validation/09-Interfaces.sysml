package InterfacesValidation {
    port def DataOut {
        out item payload;
    }
    port def DataIn {
        in item payload;
    }
    interface def DataLink {
        end source : DataOut;
        end sink : DataIn;
    }
    part def Sender {
        port txPort : DataOut;
    }
    part def Receiver {
        port rxPort : DataIn;
    }
}
