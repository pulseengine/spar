// SysML v2 Training: Connection Usages
// Representative of Systems-Modeling/SysML-v2-Release sysml/src/training/04-*

package ConnectionUsageTraining {

    port def SensorOut {
        out item reading;
    }

    port def ProcessorIn {
        in item reading;
    }

    connection def SensorLink {
        connect source.reading to target.reading;
    }

    part def Sensor {
        port out1 : SensorOut;
    }

    part def Processor {
        port in1 : ProcessorIn;
    }

    part def SensorSystem {
        part sensor1 : Sensor;
        part sensor2 : Sensor;
        part processor : Processor;

        connect sensor1.out1 to processor.in1;
        connect sensor2.out1 to processor.in1;
    }
}
