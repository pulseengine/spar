package PortsValidation {
    port def SensorPort {
        out item data;
    }
    port def ActuatorPort {
        in item command;
    }
    part def Sensor {
        out port sOut : SensorPort;
    }
    part def Actuator {
        in port aIn : ActuatorPort;
    }
}
