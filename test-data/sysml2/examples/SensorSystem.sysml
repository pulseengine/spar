// SysML v2 Example: Sensor System
// Comprehensive example exercising multiple SysML v2 features

package SensorSystemExample {
    doc /* A comprehensive sensor system model. */

    // Attribute definitions
    attribute def Temperature;
    attribute def Pressure;
    attribute def Humidity;
    attribute def Voltage;

    // Port definitions with directions
    port def EnvironmentalSensorPort {
        out item temperature : Temperature;
        out item pressure : Pressure;
        out item humidity : Humidity;
    }

    port def PowerPort {
        in item voltage : Voltage;
    }

    port def DataBusPort {
        inout item data;
    }

    // Connection definitions
    connection def SensorDataLink {
        connect source.data to target.data;
    }

    connection def PowerLink {
        connect source.voltage to target.voltage;
    }

    // Part definitions
    part def EnvironmentalSensor {
        doc "Multi-parameter environmental sensor"
        port sensorOut : EnvironmentalSensorPort;
        port power : PowerPort;
        attribute accuracy : Real = 0.01;
        attribute sampleRate : Real = 100;
    }

    part def DataAggregator {
        doc "Aggregates data from multiple sensors"
        port dataIn [1..*] : DataBusPort;
        port dataOut : DataBusPort;
        attribute bufferSize : Real = 1024;
    }

    part def PowerSupply {
        doc "System power supply"
        port powerOut [1..*] : PowerPort;
        attribute voltage : Voltage;
    }

    // Abstract controller
    abstract part def Controller {
        port dataIn : DataBusPort;
        port commandOut : DataBusPort;
        attribute processingFrequency : Real;
    }

    part def SensorController :> Controller {
        attribute processingFrequency = 1000;
    }

    // System definition with connections
    part def SensorSystem {
        doc "Complete sensor system with power and data"

        part sensors [4] : EnvironmentalSensor;
        part aggregator : DataAggregator;
        part controller : SensorController;
        part psu : PowerSupply;

        // Data connections
        connect sensors.sensorOut to aggregator.dataIn;
        connect aggregator.dataOut to controller.dataIn;
    }

    // Requirements
    requirement def SensorReq {
        doc "Sensor system requirements"
    }

    requirement accuracyReq : SensorReq {
        doc "All sensors must have accuracy better than 0.1%"
    }

    requirement latencyReq : SensorReq {
        doc "End-to-end latency must be below 10ms"
    }

    // Traceability
    satisfy accuracyReq by EnvironmentalSensor;
    verify latencyReq by SensorSystem;
}
