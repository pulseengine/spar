package ConnectionsValidation {
    port def OutPort {
        out item data;
    }
    port def InPort {
        in item data;
    }
    part def Producer {
        out port pOut : OutPort;
    }
    part def Consumer {
        in port cIn : InPort;
    }
    part def System {
        part producer : Producer;
        part consumer : Consumer;
        connect producer.pOut to consumer.cIn;
    }
}
