// SysML v2 Validation: Ports
// Based on Systems-Modeling/SysML-v2-Release sysml/src/validation

package PortsValidation {

    attribute def Temperature;
    attribute def Pressure;

    port def SensorPort {
        out item sensorData;
    }

    port def ProcessorPort {
        in item sensorData;
    }

    port def BidirectionalPort {
        inout item data;
    }

    port def EnvironmentalPort {
        out item temperature : Temperature;
        out item pressure : Pressure;
    }

    part def Sensor {
        port sensorOut : SensorPort;
    }

    part def Processor {
        port processorIn : ProcessorPort;
    }

    part def Controller {
        in port commandIn;
        out port statusOut;
        inout port dataPort : BidirectionalPort;
    }
}
