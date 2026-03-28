// SysML v2 Training: Flow Usages
// Representative of Systems-Modeling/SysML-v2-Release sysml/src/training/10-*

package FlowUsageTraining {

    attribute def Temperature;
    attribute def Pressure;

    port def SensorPort {
        out item temperature : Temperature;
        out item pressure : Pressure;
    }

    port def MonitorPort {
        in item temperature : Temperature;
        in item pressure : Pressure;
    }

    part def TemperatureSensor {
        port sensorOut : SensorPort;
    }

    part def Monitor {
        port monitorIn : MonitorPort;
    }

    part def MonitoringSystem {
        part tempSensor : TemperatureSensor;
        part monitor : Monitor;
        connect tempSensor.sensorOut to monitor.monitorIn;
    }
}
